//! Final jump landing for a pinned requirement row.
//!
//! A jump pin is navigation only: it never changes a verdict or a byte.
//! It resolves after every planned write and route, against the final
//! staged texts:
//!
//! - **`carried_task`**: the pinned row was carried, so it lands on the
//!   task it became (never on its `[>]` stub).
//! - **`moved_with_ancestor`**: an ancestor was carried, so the row moved
//!   with that branch. The carry's move map gives its line inside the new
//!   task block; the block's final line comes from its block ID.
//! - **`requirement`**: otherwise (met, dropped, or under a dropped
//!   ancestor) the row stays in its owner, found by its sibling path and
//!   clean text inside the owner's final block.
//!
//! Every landing is anchored on an exact identity: a block ID, or a
//! structural path inside one uniquely identified owner. Label text only
//! confirms a structural match. Inbox-route moves forward the owner or
//! the created task to its destination note. A target that cannot be
//! found refuses as `req_jump_lost`; one with two candidates, or whose
//! move map disagrees with the structure, refuses as
//! `req_jump_ambiguous`. Either refusal happens before commit.

use std::collections::BTreeMap;

use super::{
    req_close::ReqCloseOwner,
    req_placement::ReqPlacementError,
    req_verdicts::{ReqRowProvenance, ReqVerdict},
    req_writes::{ReqWriteOutcome, ReqWritePlan},
};
use crate::native::{
    note_tasks::{self, BlockIdLookup, NoteTask, NoteTaskSettings},
    task_reqs,
};

/// Which identity the landing followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReqLandingKind {
    CarriedTask,
    MovedWithAncestor,
    Requirement,
}

impl ReqLandingKind {
    #[cfg(test)]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::CarriedTask => "carried_task",
            Self::MovedWithAncestor => "moved_with_ancestor",
            Self::Requirement => "requirement",
        }
    }
}

/// The resolved landing for one pinned requirement row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReqLanding {
    pub(crate) key: String,
    pub(crate) kind: ReqLandingKind,
    /// The row's effective outcome: its own verdict, or the structural
    /// ancestor verdict that took its branch. Default rows are Met.
    pub(crate) outcome: ReqVerdict,
    /// Vault-relative note of the final line.
    pub(crate) path: String,
    /// 1-based final line.
    pub(crate) line: usize,
    /// The created task's block ID for a `carried_task` landing.
    pub(crate) block_id: Option<String>,
}

/// One inbox-route move applied after the requirement writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReqRouteMove {
    pub(crate) source: String,
    pub(crate) dest: String,
    pub(crate) old_block_id: Option<String>,
    pub(crate) new_block_id: Option<String>,
}

/// Everything landing resolution reads.
pub(crate) struct ReqLandingInput<'a> {
    pub(crate) target: &'a ReqRowProvenance,
    /// The frozen closing owners. Their full forests (not only the
    /// card's Pending rows) carry ancestry through hand-ticked rows.
    pub(crate) owners: &'a [ReqCloseOwner],
    pub(crate) plan: &'a ReqWritePlan,
    pub(crate) routes: &'a [ReqRouteMove],
    pub(crate) settings: &'a NoteTaskSettings,
}

/// Resolve a pinned requirement row against the final staged texts.
/// `final_text` returns a note's final text by vault-relative path.
pub(crate) fn resolve_req_landing(
    input: &ReqLandingInput<'_>,
    final_text: &dyn Fn(&str) -> Option<String>,
) -> Result<ReqLanding, ReqPlacementError> {
    let target = input.target;
    let lost = || ReqPlacementError::JumpLost {
        key: target.key.clone(),
    };
    let ambiguous = || ReqPlacementError::JumpAmbiguous {
        key: target.key.clone(),
    };
    let outcome_for = |key: &str| -> Option<&ReqWriteOutcome> {
        input
            .plan
            .outcomes
            .iter()
            .find(|outcome| outcome.row_key == key)
    };

    let own = outcome_for(&target.key);
    if let Some(outcome) = own
        && let Some(carried) = outcome.carried.as_ref()
    {
        let located = locate_block(
            &carried.path,
            &carried.block_id,
            input,
            final_text,
            target,
        )?;
        return Ok(ReqLanding {
            key: target.key.clone(),
            kind: ReqLandingKind::CarriedTask,
            outcome: outcome.verdict,
            path: located.path,
            line: located.task.line_index + 1,
            block_id: Some(located.block_id),
        });
    }

    let owner = input
        .owners
        .iter()
        .find(|owner| owner.key == target.owner_key)
        .ok_or_else(lost)?;
    let rows: BTreeMap<&str, (&Option<String>, &str, usize)> = owner
        .nodes
        .iter()
        .map(|node| {
            (
                node.key.as_str(),
                (&node.parent_key, node.path.as_str(), node.line),
            )
        })
        .collect();
    let mut ancestor_key = target.parent_key.clone();
    let mut structural: Option<(&ReqWriteOutcome, &str, usize)> = None;
    while let Some(key) = ancestor_key {
        let Some((parent, path, line)) = rows.get(key.as_str()).copied() else {
            return Err(lost());
        };
        if let Some(outcome) = outcome_for(&key)
            && outcome.verdict != ReqVerdict::Met
        {
            structural = Some((outcome, path, line));
            break;
        }
        ancestor_key = parent.clone();
    }

    if let Some((ancestor, ancestor_path, ancestor_line)) = structural
        && let Some(carried) = ancestor.carried.as_ref()
    {
        let located = locate_block(
            &carried.path,
            &carried.block_id,
            input,
            final_text,
            target,
        )?;
        let rebased =
            rebased_path(&target.path, ancestor_path).ok_or_else(lost)?;
        let structural_line = req_line(
            &located.text,
            &located.task,
            &rebased,
            &target.text,
            input.settings,
        )
        .ok_or_else(lost)?;
        // Compose the carry's move map: the row's offset under its carried
        // head in the frozen forest names its `from` line at write time;
        // `to` is relative to the created task line, which only shifts as a
        // whole afterwards.
        let offset = target.line.checked_sub(ancestor_line).ok_or_else(lost)?;
        let from = ancestor.source_line + offset;
        if let Some(line_move) = ancestor
            .moves
            .iter()
            .find(|line_move| line_move.from == from)
        {
            let relative = line_move.to as isize - carried.line as isize;
            let mapped = located.task.line_index as isize + relative;
            if mapped != structural_line as isize {
                return Err(ambiguous());
            }
        }
        return Ok(ReqLanding {
            key: target.key.clone(),
            kind: ReqLandingKind::MovedWithAncestor,
            outcome: ancestor.verdict,
            path: located.path,
            line: structural_line + 1,
            block_id: None,
        });
    }

    let outcome = match (own, structural) {
        (Some(outcome), _) => outcome.verdict,
        (None, Some((ancestor, _, _))) => ancestor.verdict,
        (None, None) => ReqVerdict::Met,
    };
    let owner_text = owner.text.as_str();
    let mut found: Vec<(String, usize)> = Vec::new();
    if target.owner_block_id.is_empty() {
        for note in candidate_notes(&target.owner_path, input) {
            let Some(text) = final_text(&note) else {
                continue;
            };
            let scan = note_tasks::scan(&text, input.settings);
            for task in scan.tasks() {
                if task.block_id.is_some() || task.description != owner_text {
                    continue;
                }
                if let Some(line) = req_line(
                    &text,
                    task,
                    &target.path,
                    &target.text,
                    input.settings,
                ) {
                    found.push((note.clone(), line));
                }
            }
        }
    } else {
        let located = locate_block(
            &target.owner_path,
            &target.owner_block_id,
            input,
            final_text,
            target,
        )?;
        if let Some(line) = req_line(
            &located.text,
            &located.task,
            &target.path,
            &target.text,
            input.settings,
        ) {
            found.push((located.path, line));
        }
    }
    match found.len() {
        0 => Err(lost()),
        1 => {
            let (path, line) = found.remove(0);
            Ok(ReqLanding {
                key: target.key.clone(),
                kind: ReqLandingKind::Requirement,
                outcome,
                path,
                line: line + 1,
                block_id: None,
            })
        }
        _ => Err(ambiguous()),
    }
}

struct LocatedBlock {
    path: String,
    block_id: String,
    text: String,
    task: NoteTask,
}

/// Find a block by exact identity in its note, else in the destinations
/// of inbox routes out of that note (renamed when the route renamed it).
fn locate_block(
    path: &str,
    block_id: &str,
    input: &ReqLandingInput<'_>,
    final_text: &dyn Fn(&str) -> Option<String>,
    target: &ReqRowProvenance,
) -> Result<LocatedBlock, ReqPlacementError> {
    let lookup = |note: &str, id: &str| -> Result<Option<LocatedBlock>, ()> {
        let Some(text) = final_text(note) else {
            return Ok(None);
        };
        let scan = note_tasks::scan(&text, input.settings);
        match scan.by_block_id(id) {
            BlockIdLookup::Found(task) => {
                let task = task.clone();
                Ok(Some(LocatedBlock {
                    path: note.to_string(),
                    block_id: id.to_string(),
                    text,
                    task,
                }))
            }
            BlockIdLookup::Duplicate(_) => Err(()),
            BlockIdLookup::NotATask { .. } | BlockIdLookup::Missing => Ok(None),
        }
    };
    let ambiguous = || ReqPlacementError::JumpAmbiguous {
        key: target.key.clone(),
    };
    if let Some(found) = lookup(path, block_id).map_err(|()| ambiguous())? {
        return Ok(found);
    }
    let mut found = Vec::new();
    for route in input.routes.iter().filter(|route| route.source == path) {
        let id = if route.old_block_id.as_deref() == Some(block_id) {
            route.new_block_id.as_deref().unwrap_or(block_id)
        } else {
            block_id
        };
        if found.iter().any(|located: &LocatedBlock| {
            located.path == route.dest && located.block_id == id
        }) {
            continue;
        }
        if let Some(located) =
            lookup(&route.dest, id).map_err(|()| ambiguous())?
        {
            found.push(located);
        }
    }
    match found.len() {
        0 => Err(ReqPlacementError::JumpLost {
            key: target.key.clone(),
        }),
        1 => Ok(found.remove(0)),
        _ => Err(ambiguous()),
    }
}

/// The owner's note plus every inbox-route destination out of it.
fn candidate_notes(path: &str, input: &ReqLandingInput<'_>) -> Vec<String> {
    let mut notes = vec![path.to_string()];
    for route in input.routes.iter().filter(|route| route.source == path) {
        if !notes.contains(&route.dest) {
            notes.push(route.dest.clone());
        }
    }
    notes
}

/// A pinned descendant's sibling path under the task its carried
/// ancestor became: the ancestor's own components drop off the front.
fn rebased_path(path: &str, ancestor_path: &str) -> Option<String> {
    let rest = path.strip_prefix(ancestor_path)?.strip_prefix('.')?;
    (!rest.is_empty()).then(|| rest.to_string())
}

/// 0-based line of the req at `path` with clean `text` under `task`.
fn req_line(
    contents: &str,
    task: &NoteTask,
    path: &str,
    text: &str,
    settings: &NoteTaskSettings,
) -> Option<usize> {
    let mut matches = task_reqs::req_forest(contents, task, settings)
        .into_iter()
        .filter(|node| task_reqs::req_path(node) == path && node.text == text);
    let node = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(node.line_index)
}

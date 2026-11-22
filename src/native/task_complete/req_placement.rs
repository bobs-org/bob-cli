//! Carried-task pull-in placement for the requirement close card.
//!
//! A requirement carried with pull-in (`carry_pulled`) becomes a task that
//! this capture created. Placement links it **once** at the ordinary
//! successor destination of its owner (`docs/task-dependencies.md` §12.3):
//! the shared closing target for a Pomodoro close, else the owner's slot
//! anchor (or the root's, `inherit`), else the implicit destination, each
//! passed through the same D7 type routing ordinary offers use. The task is
//! raised to Next and is **never** freshness-stamped: it was born in this
//! capture, so it gets no `[fresh::]`, `[unblocked::]`, or `[keeps::]`.
//! Carry without pull-in leaves the task unlinked and Ready.
//!
//! A pulled carry with no destination refuses the whole batch instead of
//! silently downgrading to an unlinked carry. Placement is pure: it reads
//! the caller's staged day and note texts and returns postimages. The
//! capture adapter stages them through the batch planner, so the single
//! commit and its disk-preimage guards still own atomicity.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use chrono::NaiveDate;

use super::{
    req_close::ReqCloseOwner,
    req_writes::ReqWritePlan,
    successors::{
        insert_successor_links, resolve_closing_target,
        resolve_implicit_destination, route_created_successor, slot_target,
        successor_link_text, successor_new_status, ClosingContext,
        ClosingContinuation, CompletedTask, DayView, EntryRole,
        OfferDestination, SuccessorLinkReport, SuccessorPlacement,
        SuccessorPlan, SuccessorTarget,
    },
};
use crate::native::{
    capture::line_spans,
    capture_task_toggle::set_task_line_status,
    note_tasks::{self, BlockIdLookup, NoteTaskSettings},
    vault_links::NoteIndex,
};

/// One task created by a `carry_pulled` verdict, in card order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReqPulledTask {
    pub(crate) row_key: String,
    pub(crate) owner_path: PathBuf,
    pub(crate) owner_block_id: String,
    /// The created task's vault-relative note.
    pub(crate) path: PathBuf,
    pub(crate) block_id: String,
}

/// Every pulled carry of a write plan, in card order: closing-owner order,
/// then requirement document order. The write pass runs bottom-to-top, so
/// its outcome order is not the order a reader expects links in.
pub(crate) fn pulled_tasks(
    owners: &[ReqCloseOwner],
    plan: &ReqWritePlan,
) -> Vec<ReqPulledTask> {
    let mut pulled: Vec<(usize, Vec<usize>, ReqPulledTask)> = plan
        .outcomes
        .iter()
        .filter(|outcome| outcome.pulled)
        .filter_map(|outcome| {
            let carried = outcome.carried.as_ref()?;
            let (position, owner) = owners
                .iter()
                .enumerate()
                .find(|(_, owner)| owner.key == outcome.owner_key)?;
            Some((
                position,
                path_components(&outcome.path),
                ReqPulledTask {
                    row_key: outcome.row_key.clone(),
                    owner_path: PathBuf::from(&owner.path),
                    owner_block_id: owner.block_id.clone(),
                    path: PathBuf::from(&carried.path),
                    block_id: carried.block_id.clone(),
                },
            ))
        })
        .collect();
    pulled.sort_by(|left, right| (left.0, &left.1).cmp(&(right.0, &right.1)));
    pulled.into_iter().map(|(_, _, task)| task).collect()
}

fn path_components(path: &str) -> Vec<usize> {
    path.split('.')
        .map(|part| part.parse::<usize>().unwrap_or(usize::MAX))
        .collect()
}

/// The closing context to use after this gesture's ordinary successors
/// were inserted. When they materialized the closing target (for
/// example a created placeholder), carried links join that entry rather
/// than creating a second one.
pub(crate) fn closing_after_successors(
    closing: &ClosingContext,
    plan: &SuccessorPlan,
) -> ClosingContext {
    let mut adjusted = closing.clone();
    if let Some(continuation) = plan.closing_continuation.as_ref() {
        adjusted.continuation = Some(ClosingContinuation {
            line: continuation.line,
            created: continuation.created,
        });
    }
    adjusted
}

/// Everything placement reads. `day_text` is the staged day text after
/// this gesture's ordinary successor insertion and before retirement;
/// `closing` must already be adjusted with [`closing_after_successors`].
pub(crate) struct ReqLinkInput<'a> {
    /// Vault root for D7 type routing; `None` keeps pure tests
    /// independent of the host configuration, as in `SuccessorInput`.
    pub(crate) bob_dir: Option<&'a Path>,
    pub(crate) today: NaiveDate,
    pub(crate) day_text: Option<&'a str>,
    pub(crate) day_relative: &'a Path,
    pub(crate) index: &'a NoteIndex,
    /// The gesture's completed set C, for slot anchors and `inherit`.
    pub(crate) completed: &'a [CompletedTask],
    pub(crate) closing: Option<&'a ClosingContext>,
    /// Current staged texts of the created tasks' notes, keyed by
    /// vault-relative path.
    pub(crate) notes: &'a BTreeMap<PathBuf, String>,
    pub(crate) settings: &'a NoteTaskSettings,
}

/// One linked carried task, for receipts and the capture result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReqLinkReport {
    pub(crate) row_key: String,
    pub(crate) path: PathBuf,
    pub(crate) block_id: String,
    pub(crate) previous_status_symbol: char,
    pub(crate) status_symbol: char,
    pub(crate) destination: OfferDestination,
    /// True when D7 type routing moved the default destination.
    pub(crate) routed: bool,
    pub(crate) link: SuccessorLinkReport,
}

/// Postimages and link reports for every pulled carry.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ReqLinkPlan {
    /// The day text with every carried link inserted, when one was.
    pub(crate) day_text: Option<String>,
    /// Changed created-task notes (status raised to Next), by path. When a
    /// created task lives in the day file itself, its edit rides only in
    /// `day_text`.
    pub(crate) notes: BTreeMap<PathBuf, String>,
    pub(crate) links: Vec<ReqLinkReport>,
}

/// Refusal from placement or jump resolution. Any refusal stages nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReqPlacementError {
    /// A pulled carry has no ordinary successor destination.
    NoDestination { row_key: String },
    /// A created task vanished between the write and the placement.
    Stale { row_key: String },
    /// A pinned requirement's final line cannot be found.
    JumpLost { key: String },
    /// A pinned requirement's final line has more than one candidate.
    JumpAmbiguous { key: String },
}

impl ReqPlacementError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::NoDestination { .. } => "req_pull_in_no_destination",
            Self::Stale { .. } => "req_close_stale",
            Self::JumpLost { .. } => "req_jump_lost",
            Self::JumpAmbiguous { .. } => "req_jump_ambiguous",
        }
    }
}

impl std::fmt::Display for ReqPlacementError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoDestination { row_key } => write!(
                formatter,
                "no open Pomodoro to pull the carried requirement into: {row_key}"
            ),
            Self::Stale { row_key } => write!(
                formatter,
                "the carried requirement's new task changed: {row_key}"
            ),
            Self::JumpLost { key } => write!(
                formatter,
                "the pinned requirement has no final line: {key}"
            ),
            Self::JumpAmbiguous { key } => write!(
                formatter,
                "the pinned requirement's final line is ambiguous: {key}"
            ),
        }
    }
}

/// Link every pulled carry at its owner's ordinary successor destination,
/// raise it to Next, and never stamp freshness.
pub(crate) fn plan_req_links(
    input: &ReqLinkInput<'_>,
    pulled: &[ReqPulledTask],
) -> Result<ReqLinkPlan, ReqPlacementError> {
    if pulled.is_empty() {
        return Ok(ReqLinkPlan::default());
    }
    let day_base = input.day_text.unwrap_or_default();
    let day = DayView::parse(day_base, input.day_relative, input.index);
    let closing_target = input.closing.map(|closing| {
        resolve_closing_target(
            day_base,
            input.day_relative,
            input.index,
            closing,
        )
    });

    // Status edits first: they are in-place, so placement coordinates
    // computed on the day text stay valid even when a created task lives
    // in the day file itself.
    let mut notes: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut previous: Vec<char> = Vec::with_capacity(pulled.len());
    for task in pulled {
        let current = notes
            .get(&task.path)
            .cloned()
            .or_else(|| input.notes.get(&task.path).cloned())
            .or_else(|| {
                (task.path == input.day_relative).then(|| day_base.to_string())
            })
            .ok_or_else(|| ReqPlacementError::Stale {
                row_key: task.row_key.clone(),
            })?;
        let (edited, status) =
            raise_to_next(&current, &task.block_id, input.settings)
                .ok_or_else(|| ReqPlacementError::Stale {
                    row_key: task.row_key.clone(),
                })?;
        previous.push(status);
        notes.insert(task.path.clone(), edited);
    }

    let mut placements = Vec::new();
    let mut destinations = Vec::new();
    let mut link_positions = Vec::new();
    for (position, task) in pulled.iter().enumerate() {
        // Link once: a created task already live in an open entry keeps
        // that link and gains no second bullet.
        if day
            .live
            .contains_key(&(task.path.clone(), task.block_id.clone()))
        {
            continue;
        }
        let (target, destination) = default_destination(
            input,
            &day,
            day_base,
            closing_target.as_ref(),
            task,
        )
        .ok_or_else(|| ReqPlacementError::NoDestination {
            row_key: task.row_key.clone(),
        })?;
        let (target, destination, routed) = match input.bob_dir {
            Some(bob_dir) => {
                let task_line = input
                    .notes
                    .get(&task.path)
                    .map(String::as_str)
                    .or_else(|| {
                        (task.path == input.day_relative).then_some(day_base)
                    })
                    .and_then(|text| {
                        raw_line_for_block(text, &task.block_id, input.settings)
                    })
                    .unwrap_or_default();
                let routed = route_created_successor(
                    bob_dir,
                    input.today,
                    day_base,
                    input.day_relative,
                    input.index,
                    &task.path,
                    &task_line,
                    target,
                    destination,
                );
                if routed.refusal.is_some() {
                    return Err(ReqPlacementError::NoDestination {
                        row_key: task.row_key.clone(),
                    });
                }
                (routed.target, routed.destination, routed.routed)
            }
            None => (target, destination, false),
        };
        placements.push(SuccessorPlacement {
            target,
            block_link: successor_link_text(
                &task.path,
                &task.block_id,
                input.day_relative,
                input.index,
            ),
        });
        destinations.push((destination, routed));
        link_positions.push(position);
    }

    let day_with_edits = notes.remove(input.day_relative);
    let insertion_base = day_with_edits.as_deref().unwrap_or(day_base);
    let (day_text, placed) = if placements.is_empty() {
        (day_with_edits, Vec::new())
    } else {
        let (text, placed) = insert_successor_links(
            insertion_base,
            input.day_relative,
            input.index,
            placements.clone(),
        );
        (Some(text), placed)
    };

    let mut links = Vec::new();
    for (((position, placement), (destination, routed)), placed) in
        link_positions
            .iter()
            .zip(&placements)
            .zip(destinations)
            .zip(placed)
    {
        let task = &pulled[*position];
        let previous_status_symbol = previous[*position];
        let closing_created = closing_target
            .as_ref()
            .is_some_and(|resolved| !routed && resolved.entry_created);
        links.push(ReqLinkReport {
            row_key: task.row_key.clone(),
            path: task.path.clone(),
            block_id: task.block_id.clone(),
            previous_status_symbol,
            status_symbol: successor_new_status(previous_status_symbol),
            link: SuccessorLinkReport {
                entry_name: destination.entry_name.clone(),
                entry_line: placed.entry_line,
                line: placed.line,
                block_link: placement.block_link.clone(),
                entry_created: placed.entry_created || closing_created,
                next_up: placed.next_up,
                block_id_created: false,
            },
            destination,
            routed,
        });
    }
    // Drop unchanged note texts so callers stage only real edits.
    notes.retain(|path, text| input.notes.get(path) != Some(text));
    Ok(ReqLinkPlan {
        day_text: day_text.filter(|text| text != day_base),
        notes,
        links,
    })
}

/// The ordinary successor destination for one created task before D7.
fn default_destination(
    input: &ReqLinkInput<'_>,
    day: &DayView,
    day_text: &str,
    closing_target: Option<&super::successors::ResolvedClosingTarget>,
    task: &ReqPulledTask,
) -> Option<(SuccessorTarget, OfferDestination)> {
    let destination =
        |name: String, role: EntryRole, fallback: bool| OfferDestination {
            day_file: input.day_relative.to_path_buf(),
            entry_name: name,
            entry_role: role,
            fallback,
        };
    // No day file means no ledger: there is no ordinary destination.
    input.day_text?;
    if let Some(resolved) = closing_target {
        let role = if resolved.entry_created {
            EntryRole::Created
        } else {
            EntryRole::Closing
        };
        return Some((
            resolved.target.clone(),
            destination(resolved.name.clone(), role, false),
        ));
    }
    if let Some(anchor) = day.anchor_for_owner(
        input.completed,
        &task.owner_path,
        &task.owner_block_id,
    ) {
        return Some((
            slot_target(&anchor),
            destination(
                day.entry_name(anchor.entry_index()),
                EntryRole::Slot,
                false,
            ),
        ));
    }
    let implicit = resolve_implicit_destination(
        day_text,
        input.day_relative,
        input.index,
    )?;
    Some((
        implicit.target,
        destination(implicit.entry_name, EntryRole::Implicit, true),
    ))
}

/// Raise the task with `block_id` to Next in place. Returns the edited
/// text and the task's previous status. No freshness field is written.
fn raise_to_next(
    contents: &str,
    block_id: &str,
    settings: &NoteTaskSettings,
) -> Option<(String, char)> {
    let scan = note_tasks::scan(contents, settings);
    let BlockIdLookup::Found(task) = scan.by_block_id(block_id) else {
        return None;
    };
    let previous = task.status_symbol;
    let next = successor_new_status(previous);
    if next == previous {
        return Some((contents.to_string(), previous));
    }
    let spans = line_spans(contents);
    let span = spans.get(task.line_index)?;
    let edited = set_task_line_status(span.text, next)?;
    let start = match task.line_index {
        0 => 0,
        index => spans.get(index - 1)?.end,
    };
    let mut updated = String::with_capacity(contents.len());
    updated.push_str(&contents[..start]);
    updated.push_str(&edited);
    updated.push_str(&contents[start + span.text.len()..]);
    Some((updated, previous))
}

fn raw_line_for_block(
    contents: &str,
    block_id: &str,
    settings: &NoteTaskSettings,
) -> Option<String> {
    let scan = note_tasks::scan(contents, settings);
    let BlockIdLookup::Found(task) = scan.by_block_id(block_id) else {
        return None;
    };
    line_spans(contents)
        .get(task.line_index)
        .map(|span| span.text.to_string())
}

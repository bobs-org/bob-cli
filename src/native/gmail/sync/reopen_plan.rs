//! Pure local planner for reopening closed Gmail Tasks.
//!
//! Consumes phase-1 candidates and a staged [`VaultSnapshot`]. It returns
//! overlay postimages and typed outcomes without network, writes, locks, or
//! commits. The later transaction phase applies these edits.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;

use super::{
    archive_scan::{
        ArchiveLinkTask, ArchiveTaskClass, ArchiveThreadEvidence,
        POSITIVE_CLOSED,
    },
    hold::{self, ReopenLiftRequest},
    reopen::PreparedReopenCandidate,
    scan::parse_task_row,
    ThreadKey,
};
use crate::native::{
    capture::{
        leading_spaces_or_tabs_len, line_spans, list_item_body,
        nearest_shallower_list_item_parent,
    },
    capture_schedule_log, collect_done, markdown,
    projects::has_trailing_prj_anchor,
    task_complete::is_recurring_task_line,
    task_fields,
    task_move::{
        plan_task_move, reopen_task_root_line, IdCollisionPolicy,
        MovePlacement, MoveRequest, MoveSelector, PlanError, VaultSnapshot,
    },
};

/// Direct-child marker for the Gmail reply reopen history.
pub(crate) const REOPEN_LOG_MARKER: &str = "📬 **REOPEN LOG**";

/// One candidate the planner can apply without the body-bearing save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReopenPlanInput {
    pub(crate) key: ThreadKey,
    pub(crate) message_id: String,
    pub(crate) message_at: DateTime<Utc>,
    pub(crate) sender: String,
    pub(crate) bounced: bool,
    pub(crate) evidence: ArchiveThreadEvidence,
}

impl From<&PreparedReopenCandidate> for ReopenPlanInput {
    fn from(candidate: &PreparedReopenCandidate) -> Self {
        Self {
            key: candidate.key.clone(),
            message_id: candidate.message_id.clone(),
            message_at: candidate.message_at,
            sender: candidate.sender.clone(),
            bounced: candidate.bounced,
            evidence: candidate.evidence.clone(),
        }
    }
}

/// Timezone, freshness, and Blocked-status inputs that stay I/O-free.
#[derive(Debug, Clone)]
pub(crate) struct ReopenPlanContext {
    pub(crate) timezone: Tz,
    pub(crate) today: NaiveDate,
    pub(crate) blocked_ok: bool,
    pub(crate) blocked_reason: Option<String>,
    /// `(slash path, block id)` identities linked under today's open
    /// Pomodoros, used only for the WRITE-skip `is_today` bit.
    pub(crate) today_identities: BTreeSet<(String, String)>,
}

/// How one candidate was handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReopenDisposition {
    Applied,
    NoOp,
    PermanentRefusal,
    TransientDeferred,
}

/// Post-restore (or in-place) identity of the touched task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReopenTargetIdentity {
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) block_id: Option<String>,
    pub(crate) restored: bool,
}

/// Typed outcome for one candidate, including marker intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReopenCandidateOutcome {
    pub(crate) key: ThreadKey,
    pub(crate) message_id: String,
    pub(crate) disposition: ReopenDisposition,
    pub(crate) reason: String,
    pub(crate) hint: Option<String>,
    pub(crate) pending_marker: bool,
    pub(crate) target: Option<ReopenTargetIdentity>,
    pub(crate) warnings: Vec<String>,
}

/// Overlay postimages plus one outcome per candidate, in input order.
#[derive(Debug, Clone)]
pub(crate) struct ReopenPlan {
    pub(crate) snapshot: VaultSnapshot,
    pub(crate) outcomes: Vec<ReopenCandidateOutcome>,
}

#[derive(Clone)]
struct Relocation {
    path: PathBuf,
    line_index: usize,
    block_id: Option<String>,
    in_done: bool,
}

struct Located {
    path: PathBuf,
    line_index: usize,
    text: String,
    block_id: Option<String>,
    status: char,
    in_done: bool,
}

/// Plan every candidate against one evolving overlay.
pub(crate) fn plan_reopens(
    mut snapshot: VaultSnapshot,
    candidates: &[ReopenPlanInput],
    ctx: &ReopenPlanContext,
) -> ReopenPlan {
    let mut outcomes = Vec::with_capacity(candidates.len());
    let mut relocations: BTreeMap<(String, usize), Relocation> =
        BTreeMap::new();
    for candidate in candidates {
        let outcome = plan_one(&mut snapshot, candidate, ctx, &mut relocations);
        outcomes.push(outcome);
    }
    ReopenPlan { snapshot, outcomes }
}

/// Convenience wrapper for phase-1 [`PreparedReopenCandidate`] values.
#[allow(dead_code)]
pub(crate) fn plan_prepared_reopens(
    snapshot: VaultSnapshot,
    candidates: &[PreparedReopenCandidate],
    ctx: &ReopenPlanContext,
) -> ReopenPlan {
    let inputs: Vec<_> = candidates.iter().map(ReopenPlanInput::from).collect();
    plan_reopens(snapshot, &inputs, ctx)
}

fn plan_one(
    snapshot: &mut VaultSnapshot,
    candidate: &ReopenPlanInput,
    ctx: &ReopenPlanContext,
    relocations: &mut BTreeMap<(String, usize), Relocation>,
) -> ReopenCandidateOutcome {
    let selected = match select_newest_linker(&candidate.evidence) {
        Ok(task) => task,
        Err(outcome) => return finish(candidate, outcome, None, Vec::new()),
    };
    if is_recurring_task_line(&selected.text) {
        return finish(
            candidate,
            (
                ReopenDisposition::PermanentRefusal,
                "recurring",
                Some(
                    "Gmail cannot reopen a recurring task; handle it by hand"
                        .to_string(),
                ),
                false,
            ),
            None,
            Vec::new(),
        );
    }
    if has_trailing_prj_anchor(&selected.text) {
        return finish(
            candidate,
            (
                ReopenDisposition::PermanentRefusal,
                "project_task",
                Some(
                    "Gmail cannot reopen a ^prj line; handle it by hand"
                        .to_string(),
                ),
                false,
            ),
            None,
            Vec::new(),
        );
    }
    if message_before_close(candidate, selected, ctx.timezone) {
        return finish(
            candidate,
            (ReopenDisposition::NoOp, "message_before_close", None, false),
            None,
            Vec::new(),
        );
    }

    let located = match locate_linker(snapshot, selected, relocations) {
        Ok(located) => located,
        Err(outcome) => return finish(candidate, outcome, None, Vec::new()),
    };

    // A crash or marker-write failure can leave the task's exact REOPEN LOG
    // entry durable while the marker is still pending. This applies to a
    // task Bryan has since re-closed too: finish the marker without opening
    // the task a second time.
    if snapshot.notes.get(&located.path).is_some_and(|contents| {
        log_has_message_card(
            contents,
            located.line_index,
            &mail_stem(&candidate.evidence.note),
            &candidate.message_id,
        )
    }) {
        return finish(
            candidate,
            (ReopenDisposition::NoOp, "already_logged", None, true),
            Some(ReopenTargetIdentity {
                path: slash(&located.path),
                line: located.line_index + 1,
                block_id: located.block_id.clone(),
                restored: located.in_done,
            }),
            Vec::new(),
        );
    }

    if POSITIVE_CLOSED.contains(&located.status) {
        apply_closed(snapshot, candidate, ctx, selected, located, relocations)
    } else if super::scan::OPEN_STATUSES.contains(&located.status) {
        apply_already_open(
            snapshot,
            candidate,
            ctx,
            selected,
            located,
            relocations,
        )
    } else {
        finish(
            candidate,
            (
                ReopenDisposition::TransientDeferred,
                "stale_line",
                Some(
                    "the selected task no longer has a known checkbox; retry"
                        .to_string(),
                ),
                false,
            ),
            None,
            Vec::new(),
        )
    }
}

fn apply_closed(
    snapshot: &mut VaultSnapshot,
    candidate: &ReopenPlanInput,
    ctx: &ReopenPlanContext,
    selected: &ArchiveLinkTask,
    located: Located,
    relocations: &mut BTreeMap<(String, usize), Relocation>,
) -> ReopenCandidateOutcome {
    if located.in_done {
        apply_restore(snapshot, candidate, ctx, selected, located, relocations)
    } else {
        apply_live(snapshot, candidate, ctx, selected, located, relocations)
    }
}

fn apply_restore(
    snapshot: &mut VaultSnapshot,
    candidate: &ReopenPlanInput,
    ctx: &ReopenPlanContext,
    selected: &ArchiveLinkTask,
    located: Located,
    relocations: &mut BTreeMap<(String, usize), Relocation>,
) -> ReopenCandidateOutcome {
    let selector = match &located.block_id {
        Some(id) => MoveSelector::Block(id.clone()),
        None => MoveSelector::Line(located.line_index),
    };
    let request = MoveRequest {
        source: slash(&located.path),
        dest: String::new(),
        selectors: vec![selector],
        allow_closed: true,
        placement: MovePlacement::Append,
        id_collision: IdCollisionPolicy::Rename,
        restore: true,
        reopen: true,
    };
    let plan = match plan_task_move(snapshot, &request) {
        Ok(plan) => plan,
        Err(error) => return restore_error(candidate, &error),
    };
    let Some(moved) = plan.moves.first() else {
        return finish(
            candidate,
            (
                ReopenDisposition::TransientDeferred,
                "stale_line",
                Some("restore produced no move record; retry".to_string()),
                false,
            ),
            None,
            Vec::new(),
        );
    };
    snapshot.notes.extend(plan.postimages);
    let dest_index = moved.dest_line.saturating_sub(1);
    let dest_path = moved.dest_path.clone();
    let block_id = moved.new_block_id.clone();
    match edit_open_row(
        snapshot,
        candidate,
        ctx,
        &dest_path,
        dest_index,
        block_id.clone(),
        false,
        selected,
        relocations,
        true,
    ) {
        Ok(outcome) => outcome,
        Err(outcome) => finish(candidate, outcome, None, Vec::new()),
    }
}

fn apply_live(
    snapshot: &mut VaultSnapshot,
    candidate: &ReopenPlanInput,
    ctx: &ReopenPlanContext,
    selected: &ArchiveLinkTask,
    located: Located,
    relocations: &mut BTreeMap<(String, usize), Relocation>,
) -> ReopenCandidateOutcome {
    let contents = match snapshot.notes.get(&located.path) {
        Some(contents) => contents.clone(),
        None => {
            return finish(
                candidate,
                missing_note_outcome(&located.path),
                None,
                Vec::new(),
            );
        }
    };
    let reopened = reopen_task_root_line(&located.text);
    let Some(updated) =
        replace_logical_line(&contents, located.line_index, &reopened)
    else {
        return finish(
            candidate,
            (
                ReopenDisposition::TransientDeferred,
                "stale_line",
                Some("the selected task line is gone; retry".to_string()),
                false,
            ),
            None,
            Vec::new(),
        );
    };
    snapshot.notes.insert(located.path.clone(), updated);
    match edit_open_row(
        snapshot,
        candidate,
        ctx,
        &located.path,
        located.line_index,
        located.block_id.clone(),
        located.in_done,
        selected,
        relocations,
        false,
    ) {
        Ok(outcome) => outcome,
        Err(outcome) => finish(candidate, outcome, None, Vec::new()),
    }
}

fn apply_already_open(
    snapshot: &mut VaultSnapshot,
    candidate: &ReopenPlanInput,
    ctx: &ReopenPlanContext,
    selected: &ArchiveLinkTask,
    located: Located,
    relocations: &mut BTreeMap<(String, usize), Relocation>,
) -> ReopenCandidateOutcome {
    let contents = match snapshot.notes.get(&located.path) {
        Some(contents) => contents.clone(),
        None => {
            return finish(
                candidate,
                missing_note_outcome(&located.path),
                None,
                Vec::new(),
            );
        }
    };
    if log_has_message_card(
        &contents,
        located.line_index,
        &mail_stem(&candidate.evidence.note),
        &candidate.message_id,
    ) {
        let target = ReopenTargetIdentity {
            path: slash(&located.path),
            line: located.line_index + 1,
            block_id: located.block_id.clone(),
            restored: false,
        };
        return finish(
            candidate,
            (ReopenDisposition::NoOp, "already_logged", None, true),
            Some(target),
            Vec::new(),
        );
    }
    match edit_open_row(
        snapshot,
        candidate,
        ctx,
        &located.path,
        located.line_index,
        located.block_id.clone(),
        located.in_done,
        selected,
        relocations,
        false,
    ) {
        Ok(outcome) => outcome,
        Err(outcome) => finish(candidate, outcome, None, Vec::new()),
    }
}

#[allow(clippy::too_many_arguments)]
fn edit_open_row(
    snapshot: &mut VaultSnapshot,
    candidate: &ReopenPlanInput,
    ctx: &ReopenPlanContext,
    path: &Path,
    line_index: usize,
    block_id: Option<String>,
    in_done: bool,
    selected: &ArchiveLinkTask,
    relocations: &mut BTreeMap<(String, usize), Relocation>,
    restored: bool,
) -> Result<ReopenCandidateOutcome, OutcomeParts> {
    let contents = snapshot
        .notes
        .get(path)
        .cloned()
        .ok_or_else(|| missing_note_outcome(path))?;
    let lines = line_spans(&contents);
    let line = lines
        .get(line_index)
        .map(|span| span.text.to_string())
        .ok_or((
            ReopenDisposition::TransientDeferred,
            "stale_line",
            Some("the selected task line is gone; retry".to_string()),
            false,
        ))?;
    let scope = hold::unblock_scope_for(
        path,
        &line,
        is_today(ctx, path, block_id.as_deref()),
    );
    let lifted = hold::apply_reopen_lift(
        &line,
        &ReopenLiftRequest {
            today: ctx.today,
            scope: &scope,
            blocked_ok: ctx.blocked_ok,
            blocked_reason: ctx.blocked_reason.clone(),
        },
    );
    let mut warnings = Vec::new();
    if let Some(warning) = lifted.blocker_warning.clone() {
        warnings.push(warning);
    }
    let mut working = contents;
    if lifted.changed {
        working = replace_logical_line(&working, line_index, &lifted.line)
            .ok_or((
                ReopenDisposition::TransientDeferred,
                "stale_line",
                Some("the selected task line is gone; retry".to_string()),
                false,
            ))?;
    }
    let entry = reopen_log_entry(candidate, ctx.timezone);
    if !log_has_message_card(
        &working,
        line_index,
        &mail_stem(&candidate.evidence.note),
        &candidate.message_id,
    ) {
        working = capture_schedule_log::plan_named_log_entry_insertion(
            &working,
            line_index,
            &entry,
            REOPEN_LOG_MARKER,
            is_reopen_log_marker,
            true,
        )
        .0;
    }
    snapshot.notes.insert(path.to_path_buf(), working);
    let identity = ReopenTargetIdentity {
        path: slash(path),
        line: line_index + 1,
        block_id: block_id.clone(),
        restored,
    };
    relocations.insert(
        (selected.path.clone(), selected.line),
        Relocation {
            path: path.to_path_buf(),
            line_index,
            block_id,
            in_done,
        },
    );
    Ok(finish(
        candidate,
        (ReopenDisposition::Applied, "applied", None, true),
        Some(identity),
        warnings,
    ))
}

fn restore_error(
    candidate: &ReopenPlanInput,
    error: &PlanError,
) -> ReopenCandidateOutcome {
    let (disposition, pending_marker) = match error {
        PlanError::MissingNote { .. }
        | PlanError::SelectorZero { .. }
        | PlanError::SelectorAmbiguous { .. }
        | PlanError::AmbiguousNote { .. }
        | PlanError::AmbiguousIdentity { .. }
        | PlanError::InvalidPath { .. } => {
            (ReopenDisposition::TransientDeferred, false)
        }
        _ => (ReopenDisposition::PermanentRefusal, false),
    };
    let hint = match error {
        PlanError::RestoreRecurring { .. } => Some(
            "Gmail cannot restore a recurring task; handle it by hand"
                .to_string(),
        ),
        PlanError::ProjectTask { .. } => Some(
            "Gmail cannot restore a ^prj line; handle it by hand".to_string(),
        ),
        PlanError::RestoreTombstone { .. } => Some(
            "Gmail cannot restore a merged-task tombstone; handle it by hand"
                .to_string(),
        ),
        PlanError::RestoreNoSource { .. } => Some(
            "restore this task with bob task restore --reopen, then retry"
                .to_string(),
        ),
        PlanError::NestedTask { .. }
        | PlanError::UnsupportedCheckbox { .. }
        | PlanError::DestKind { .. }
        | PlanError::DestBlocked { .. }
        | PlanError::DependsOnDestPrj { .. } => Some(
            "restore this task with bob task restore --reopen, then retry"
                .to_string(),
        ),
        _ => Some(error.to_string()),
    };
    finish(
        candidate,
        (disposition, error.code(), hint, pending_marker),
        None,
        Vec::new(),
    )
}

type OutcomeParts = (ReopenDisposition, &'static str, Option<String>, bool);

fn finish(
    candidate: &ReopenPlanInput,
    parts: OutcomeParts,
    target: Option<ReopenTargetIdentity>,
    warnings: Vec<String>,
) -> ReopenCandidateOutcome {
    let (disposition, reason, hint, pending_marker) = parts;
    ReopenCandidateOutcome {
        key: candidate.key.clone(),
        message_id: candidate.message_id.clone(),
        disposition,
        reason: reason.to_string(),
        hint,
        pending_marker,
        target,
        warnings,
    }
}

fn missing_note_outcome(path: &Path) -> OutcomeParts {
    (
        ReopenDisposition::TransientDeferred,
        "missing_note",
        Some(format!(
            "task note {} is missing from the staged snapshot; retry",
            path.display()
        )),
        false,
    )
}

/// Newest effective positively closed linker, excluding merge-retired
/// tombstones. Latest close date wins; ties prefer live over `done/`, then
/// ascending path and line. Missing close dates sort last.
pub(crate) fn select_newest_linker(
    evidence: &ArchiveThreadEvidence,
) -> Result<&ArchiveLinkTask, OutcomeParts> {
    let mut candidates: Vec<&ArchiveLinkTask> = evidence
        .tasks
        .iter()
        .filter(|task| {
            task.class == ArchiveTaskClass::ClosedPositive
                && !task.merge_retired
        })
        .collect();
    if candidates.is_empty() {
        return Err((
            ReopenDisposition::PermanentRefusal,
            "no_reopenable_linker",
            Some(
                "this thread's closed proof is only a merge tombstone; pick a task by hand"
                    .to_string(),
            ),
            false,
        ));
    }
    candidates.sort_by(|left, right| {
        close_date_rank(left)
            .cmp(&close_date_rank(right))
            .then(left.in_done.cmp(&right.in_done))
            .then(left.path.cmp(&right.path))
            .then(left.line.cmp(&right.line))
    });
    Ok(candidates[0])
}

fn close_date_rank(
    task: &ArchiveLinkTask,
) -> (u8, Option<std::cmp::Reverse<NaiveDate>>) {
    match latest_close_date(&task.text) {
        Some(date) => (0, Some(std::cmp::Reverse(date))),
        None => (1, None),
    }
}

fn latest_close_date(line: &str) -> Option<NaiveDate> {
    let completion = task_fields::inline_fields(line, "completion")
        .into_iter()
        .filter_map(|field| {
            task_fields::parse_strict_calendar_date(field.value.trim())
        });
    let cancelled = task_fields::inline_fields(line, "cancelled")
        .into_iter()
        .filter_map(|field| {
            task_fields::parse_strict_calendar_date(field.value.trim())
        });
    completion.chain(cancelled).max()
}

fn message_before_close(
    candidate: &ReopenPlanInput,
    selected: &ArchiveLinkTask,
    timezone: Tz,
) -> bool {
    let Some(close) = latest_close_date(&selected.text) else {
        return false;
    };
    let message_date =
        candidate.message_at.with_timezone(&timezone).date_naive();
    message_date < close
}

fn locate_linker(
    snapshot: &VaultSnapshot,
    selected: &ArchiveLinkTask,
    relocations: &BTreeMap<(String, usize), Relocation>,
) -> Result<Located, OutcomeParts> {
    if let Some(relocated) =
        relocations.get(&(selected.path.clone(), selected.line))
    {
        return located_at(snapshot, relocated);
    }
    let path = PathBuf::from(&selected.path);
    let contents = snapshot
        .notes
        .get(&path)
        .ok_or_else(|| missing_note_outcome(&path))?;
    let lines = line_spans(contents);
    let line_index = if let Some(block_id) = selected.block_id.as_deref() {
        find_block_id_line(&lines, block_id).ok_or((
            ReopenDisposition::TransientDeferred,
            "stale_line",
            Some(format!(
                "task ^{block_id} is missing from {}; retry",
                selected.path
            )),
            false,
        ))?
    } else {
        let index = selected.line.checked_sub(1).ok_or((
            ReopenDisposition::TransientDeferred,
            "stale_line",
            Some("the selected task line is gone; retry".to_string()),
            false,
        ))?;
        if index >= lines.len() {
            return Err((
                ReopenDisposition::TransientDeferred,
                "stale_line",
                Some("the selected task line is gone; retry".to_string()),
                false,
            ));
        }
        index
    };
    let text = lines[line_index].text.to_string();
    let Some(row) = parse_task_row(&text) else {
        return Err((
            ReopenDisposition::TransientDeferred,
            "stale_line",
            Some("the selected line is no longer a task; retry".to_string()),
            false,
        ));
    };
    if let Some(expected) = selected.block_id.as_deref() {
        let found = collect_done::trailing_block_id_in_line(&text);
        if found.as_deref() != Some(expected) {
            return Err((
                ReopenDisposition::TransientDeferred,
                "stale_line",
                Some("the selected task identity moved; retry".to_string()),
                false,
            ));
        }
    }
    Ok(Located {
        path,
        line_index,
        block_id: collect_done::trailing_block_id_in_line(&text),
        status: row.status,
        in_done: selected.in_done,
        text,
    })
}

fn located_at(
    snapshot: &VaultSnapshot,
    relocated: &Relocation,
) -> Result<Located, OutcomeParts> {
    let contents = snapshot
        .notes
        .get(&relocated.path)
        .ok_or_else(|| missing_note_outcome(&relocated.path))?;
    let lines = line_spans(contents);
    let line_index = if let Some(block_id) = relocated.block_id.as_deref() {
        find_block_id_line(&lines, block_id).unwrap_or(relocated.line_index)
    } else {
        relocated.line_index
    };
    let text = lines
        .get(line_index)
        .map(|span| span.text.to_string())
        .ok_or((
            ReopenDisposition::TransientDeferred,
            "stale_line",
            Some("the selected task line is gone; retry".to_string()),
            false,
        ))?;
    let Some(row) = parse_task_row(&text) else {
        return Err((
            ReopenDisposition::TransientDeferred,
            "stale_line",
            Some("the selected line is no longer a task; retry".to_string()),
            false,
        ));
    };
    Ok(Located {
        path: relocated.path.clone(),
        line_index,
        block_id: collect_done::trailing_block_id_in_line(&text),
        status: row.status,
        in_done: relocated.in_done,
        text,
    })
}

fn find_block_id_line(
    lines: &[crate::native::capture::LineSpan<'_>],
    block_id: &str,
) -> Option<usize> {
    lines.iter().position(|span| {
        collect_done::trailing_block_id_in_line(span.text).as_deref()
            == Some(block_id)
    })
}

fn replace_logical_line(
    contents: &str,
    line_index: usize,
    new_text: &str,
) -> Option<String> {
    let lines = line_spans(contents);
    let span = lines.get(line_index)?;
    let start = if line_index == 0 {
        0
    } else {
        lines[line_index - 1].end
    };
    let original = &contents[start..span.end];
    let ending = if original.ends_with("\r\n") {
        "\r\n"
    } else if original.ends_with('\n') {
        "\n"
    } else {
        ""
    };
    let body = new_text.strip_suffix('\r').unwrap_or(new_text);
    Some(format!(
        "{}{body}{ending}{}",
        &contents[..start],
        &contents[span.end..]
    ))
}

fn reopen_log_entry(candidate: &ReopenPlanInput, timezone: Tz) -> String {
    let local = candidate.message_at.with_timezone(&timezone);
    let date = local.date_naive();
    let when = local.format("%Y-%m-%d %H:%M");
    let stem = mail_stem(&candidate.evidence.note);
    let card = format!("[[{stem}#^m-{}|{when}]]", candidate.message_id);
    let description = if candidate.bounced {
        "bounced".to_string()
    } else {
        format!("reply from {}", candidate.sender)
    };
    format!("*{date}* — ✉️ {description} · {card}")
}

fn mail_stem(note: &str) -> String {
    Path::new(note)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| note.to_string())
}

fn log_has_message_card(
    contents: &str,
    task_line_index: usize,
    stem: &str,
    message_id: &str,
) -> bool {
    let lines = line_spans(contents);
    let Some(task_end) = lines
        .get(task_line_index)
        .map(|_| child_block_end_line(&lines, task_line_index))
    else {
        return false;
    };
    let fenced = fenced_line_indexes(&lines);
    let Some(marker) = (task_line_index + 1..=task_end).find(|&index| {
        !fenced.contains(&index)
            && is_reopen_log_marker(lines[index].text)
            && nearest_shallower_list_item_parent(&lines, index)
                == Some(task_line_index)
    }) else {
        return false;
    };
    let marker_end = child_block_end_line(&lines, marker);
    let needle = format!("[[{stem}#^m-{message_id}");
    (marker + 1..=marker_end).any(|index| {
        !fenced.contains(&index) && lines[index].text.contains(&needle)
    })
}

fn fenced_line_indexes(
    lines: &[crate::native::capture::LineSpan<'_>],
) -> BTreeSet<usize> {
    let logical: Vec<&str> = lines.iter().map(|line| line.text).collect();
    markdown::fenced_lines(&logical, 0..logical.len())
}

fn child_block_end_line(
    lines: &[crate::native::capture::LineSpan<'_>],
    parent_index: usize,
) -> usize {
    let parent_indent = leading_spaces_or_tabs_len(lines[parent_index].text);
    let mut end_index = parent_index;
    let mut index = parent_index + 1;
    while index < lines.len() {
        let text = lines[index].text;
        if text.trim().is_empty() {
            index += 1;
            continue;
        }
        if leading_spaces_or_tabs_len(text) > parent_indent {
            end_index = index;
            index += 1;
            continue;
        }
        break;
    }
    end_index
}

fn is_reopen_log_marker(line: &str) -> bool {
    let Some(rest) = list_item_body(line) else {
        return false;
    };
    let Some(after_emoji) = rest.strip_prefix('📬') else {
        return false;
    };
    let after_emoji =
        after_emoji.strip_prefix('\u{FE0F}').unwrap_or(after_emoji);
    let whitespace = leading_spaces_or_tabs_len(after_emoji);
    if whitespace == 0 {
        return false;
    }
    let rest = &after_emoji[whitespace..];
    let Some(rest) = rest.strip_prefix("**") else {
        return false;
    };
    let Some(rest) = rest.strip_prefix("REOPEN LOG") else {
        return false;
    };
    let rest = rest.strip_prefix(':').unwrap_or(rest);
    let Some(rest) = rest.strip_prefix("**") else {
        return false;
    };
    rest.bytes().all(|byte| matches!(byte, b' ' | b'\t'))
}

fn is_today(
    ctx: &ReopenPlanContext,
    path: &Path,
    block_id: Option<&str>,
) -> bool {
    let Some(block_id) = block_id else {
        return false;
    };
    ctx.today_identities
        .contains(&(slash(path), block_id.to_string()))
}

fn slash(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::super::{
        archive_scan::classify_archive_status, hold::AWAITING_TAG,
        scan::LinkPlacement,
    };
    use super::*;
    use crate::native::gmail::body::inert_single_line;

    const ACCOUNT: &str = "me@example.com";
    const THREAD: &str = "aabbccdd";
    const MESSAGE: &str = "1111aaaa";
    const TZ: Tz = chrono_tz::America::New_York;

    fn key(thread_id: &str) -> ThreadKey {
        ThreadKey::new(ACCOUNT, thread_id).unwrap()
    }

    fn at(rfc3339: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc3339)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 14).unwrap()
    }

    fn ctx() -> ReopenPlanContext {
        ReopenPlanContext {
            timezone: TZ,
            today: today(),
            blocked_ok: true,
            blocked_reason: None,
            today_identities: BTreeSet::new(),
        }
    }

    fn snap(notes: &[(&str, &str)]) -> VaultSnapshot {
        VaultSnapshot::from_notes(
            notes.iter().map(|(path, text)| {
                (PathBuf::from(*path), (*text).to_string())
            }),
        )
    }

    fn line_number(contents: &str, needle: &str) -> usize {
        contents
            .lines()
            .position(|line| line.contains(needle))
            .unwrap()
            + 1
    }

    fn closed_task(
        path: &str,
        contents: &str,
        needle: &str,
        in_done: bool,
    ) -> ArchiveLinkTask {
        let line = line_number(contents, needle);
        let text = contents
            .lines()
            .find(|row| row.contains(needle))
            .unwrap()
            .to_string();
        let status = parse_task_row(&text).unwrap().status;
        ArchiveLinkTask {
            path: path.into(),
            line,
            text: text.clone(),
            block_id: collect_done::trailing_block_id_in_line(&text),
            status,
            class: classify_archive_status(status),
            placement: LinkPlacement::TaskLine,
            in_done,
            merge_retired: false,
            via_merge: false,
            from_duplicate_note: false,
        }
    }

    fn evidence(tasks: Vec<ArchiveLinkTask>) -> ArchiveThreadEvidence {
        ArchiveThreadEvidence {
            key: key(THREAD),
            note: format!("mail/{THREAD}.md"),
            tasks,
            saved_ids: vec!["saved".into()],
            note_fingerprint: Some("sha256:fixture".into()),
            merge_unresolved: None,
            identity_uncertain: false,
            issues: Vec::new(),
        }
    }

    fn candidate(
        thread_id: &str,
        message_id: &str,
        message_at: DateTime<Utc>,
        sender: &str,
        bounced: bool,
        tasks: Vec<ArchiveLinkTask>,
    ) -> ReopenPlanInput {
        let mut body = evidence(tasks);
        body.key = key(thread_id);
        body.note = format!("mail/{thread_id}.md");
        ReopenPlanInput {
            key: key(thread_id),
            message_id: message_id.into(),
            message_at,
            sender: sender.into(),
            bounced,
            evidence: body,
        }
    }

    fn job_note(task: &str) -> String {
        format!("## Tasks\n\n{task}\n")
    }

    fn post<'a>(plan: &'a ReopenPlan, path: &str) -> &'a str {
        plan.snapshot
            .notes
            .get(Path::new(path))
            .map(String::as_str)
            .unwrap_or_else(|| panic!("missing postimage {path}"))
    }

    fn paired_source() -> String {
        "---\ntype: [[area]]\ndone_tasks: \"[[done/work_done]]\"\n---\n## Tasks\n"
            .to_string()
    }

    fn paired_archive(body: &str) -> String {
        format!("---\nparent: \"[[work]]\"\ntype: \"[[done]]\"\n---\n\n{body}")
    }

    fn message_at() -> DateTime<Utc> {
        at("2026-11-14T15:00:00Z")
    }

    #[test]
    fn reopens_x_x_upper_and_cancelled_in_place() {
        for (status, field) in [
            ('x', "[completion:: 2026-11-14]"),
            ('X', "[completion:: 2026-11-14]"),
            ('-', "[cancelled:: 2026-11-14]"),
        ] {
            let task = format!(
                "- [{status}] #task Closed [[mail/{THREAD}]] {field} [fresh:: 2026-11-10] [created:: 2026-11-01] ^t"
            );
            let job = job_note(&task);
            let linker = closed_task("job.md", &job, "Closed", false);
            let plan = plan_reopens(
                snap(&[("job.md", job.as_str())]),
                &[candidate(
                    THREAD,
                    MESSAGE,
                    message_at(),
                    "Ada",
                    false,
                    vec![linker],
                )],
                &ctx(),
            );
            let out = &plan.outcomes[0];
            assert_eq!(out.disposition, ReopenDisposition::Applied, "{status}");
            assert!(out.pending_marker);
            let dest = post(&plan, "job.md");
            assert!(dest.contains("- [ ] #task Closed"), "{dest}");
            assert!(!dest.contains("[completion::"), "{dest}");
            assert!(!dest.contains("[cancelled::"), "{dest}");
            assert!(dest.contains("[fresh:: 2026-11-10]"), "{dest}");
            assert!(dest.contains("📬 **REOPEN LOG**"), "{dest}");
            assert!(
                dest.contains(&format!(
                    "[[{THREAD}#^m-{MESSAGE}|2026-11-14 10:00]]"
                )),
                "{dest}"
            );
            assert!(dest.contains("reply from Ada"), "{dest}");
        }
    }

    #[test]
    fn newest_close_date_wins_and_missing_dates_sort_last() {
        let older = "- [x] #task Older [[mail/aabbccdd]] [completion:: 2026-11-01] ^old";
        let newer = "- [x] #task Newer [[mail/aabbccdd]] [completion:: 2026-11-10] ^new";
        let undated = "- [x] #task Undated [[mail/aabbccdd]] ^undated";
        let job = format!("## Tasks\n\n{older}\n{newer}\n{undated}\n");
        let tasks = vec![
            closed_task("job.md", &job, "Older", false),
            closed_task("job.md", &job, "Undated", false),
            closed_task("job.md", &job, "Newer", false),
        ];
        let body = evidence(tasks);
        let selected = select_newest_linker(&body).unwrap();
        assert_eq!(selected.block_id.as_deref(), Some("new"));
    }

    #[test]
    fn ties_prefer_live_then_ascending_path_and_line() {
        let live_a =
            "- [x] #task Live A [[mail/aabbccdd]] [completion:: 2026-11-10] ^a";
        let live_b =
            "- [x] #task Live B [[mail/aabbccdd]] [completion:: 2026-11-10] ^b";
        let archived = "- [x] #task Archived [[mail/aabbccdd]] [completion:: 2026-11-10] ^z";
        let job = format!("## Tasks\n\n{live_b}\n{live_a}\n");
        let done = paired_archive(&format!("{archived}\n"));
        let mut tasks = vec![
            closed_task("job.md", &job, "Live B", false),
            closed_task("job.md", &job, "Live A", false),
            closed_task("done/work_done.md", &done, "Archived", true),
        ];
        let body = evidence(tasks.clone());
        let selected = select_newest_linker(&body).unwrap();
        assert_eq!(selected.block_id.as_deref(), Some("b"));

        tasks.retain(|task| task.block_id.as_deref() != Some("b"));
        let body = evidence(tasks);
        let selected = select_newest_linker(&body).unwrap();
        assert_eq!(selected.block_id.as_deref(), Some("a"));
    }

    #[test]
    fn timezone_date_guard_blocks_earlier_messages_and_allows_same_day() {
        let task = "- [x] #task Closed [[mail/aabbccdd]] [completion:: 2026-11-14] [fresh:: 2026-11-10] ^t";
        let job = job_note(task);
        let linker = closed_task("job.md", &job, "Closed", false);
        let early = plan_reopens(
            snap(&[("job.md", job.as_str())]),
            &[candidate(
                THREAD,
                MESSAGE,
                at("2026-11-14T04:00:00Z"),
                "Ada",
                false,
                vec![linker.clone()],
            )],
            &ctx(),
        );
        assert_eq!(early.outcomes[0].disposition, ReopenDisposition::NoOp);
        assert_eq!(early.outcomes[0].reason, "message_before_close");
        assert!(!early.outcomes[0].pending_marker);
        assert_eq!(post(&early, "job.md"), job);

        let same_day = plan_reopens(
            snap(&[("job.md", job.as_str())]),
            &[candidate(
                THREAD,
                MESSAGE,
                at("2026-11-14T05:00:00Z"),
                "Ada",
                false,
                vec![linker],
            )],
            &ctx(),
        );
        assert_eq!(
            same_day.outcomes[0].disposition,
            ReopenDisposition::Applied
        );
    }

    #[test]
    fn tombstone_stays_untouched_while_via_merge_target_reopens() {
        let tomb = "- [-] #task Tombstone [[mail/aabbccdd]] [cancelled:: 2026-11-10] ^dead";
        let target = "- [x] #task Target [completion:: 2026-11-10] [fresh:: 2026-11-01] ^live";
        let job = format!("## Tasks\n\n{tomb}\n{target}\n");
        let mut tomb_task = closed_task("job.md", &job, "Tombstone", false);
        tomb_task.merge_retired = true;
        let mut target_task = closed_task("job.md", &job, "Target", false);
        target_task.via_merge = true;
        let plan = plan_reopens(
            snap(&[("job.md", job.as_str())]),
            &[candidate(
                THREAD,
                MESSAGE,
                message_at(),
                "Ada",
                false,
                vec![tomb_task, target_task],
            )],
            &ctx(),
        );
        let dest = post(&plan, "job.md");
        assert!(dest.contains("- [-] #task Tombstone"), "{dest}");
        assert!(dest.contains("[cancelled:: 2026-11-10]"), "{dest}");
        assert!(dest.contains("- [ ] #task Target"), "{dest}");
        assert!(dest.contains("📬 **REOPEN LOG**"), "{dest}");
    }

    #[test]
    fn restores_from_done_and_repairs_inbound_links() {
        let source = paired_source();
        let archive = paired_archive(
            "- [x] #task Done thing [[mail/aabbccdd]] [completion:: 2026-11-10] [fresh:: 2026-11-01] ^a\n",
        );
        let links = "See [[done/work_done#^a]].\n";
        let linker =
            closed_task("done/work_done.md", &archive, "Done thing", true);
        let plan = plan_reopens(
            snap(&[
                ("work.md", source.as_str()),
                ("done/work_done.md", archive.as_str()),
                ("links.md", links),
            ]),
            &[candidate(
                THREAD,
                MESSAGE,
                message_at(),
                "Ada",
                false,
                vec![linker],
            )],
            &ctx(),
        );
        assert_eq!(plan.outcomes[0].disposition, ReopenDisposition::Applied);
        assert_eq!(
            plan.outcomes[0].target.as_ref().map(|t| t.restored),
            Some(true)
        );
        let dest = post(&plan, "work.md");
        assert!(dest.contains("- [ ] #task Done thing"), "{dest}");
        assert!(dest.contains("📬 **REOPEN LOG**"), "{dest}");
        assert!(!dest.contains("[completion::"), "{dest}");
        let archive_post = post(&plan, "done/work_done.md");
        assert!(!archive_post.contains("Done thing"), "{archive_post}");
        let links_post = post(&plan, "links.md");
        assert!(links_post.contains("[[work#^a]]"), "{links_post}");
    }

    #[test]
    fn restore_renames_colliding_block_ids() {
        let source = format!("{}- [ ] #task Existing ^a\n", paired_source());
        let archive = paired_archive(
            "- [x] #task Incoming [[mail/aabbccdd]] [completion:: 2026-11-10] [fresh:: 2026-11-01] ^a\n",
        );
        let linker =
            closed_task("done/work_done.md", &archive, "Incoming", true);
        let plan = plan_reopens(
            snap(&[
                ("work.md", source.as_str()),
                ("done/work_done.md", archive.as_str()),
            ]),
            &[candidate(
                THREAD,
                MESSAGE,
                message_at(),
                "Ada",
                false,
                vec![linker],
            )],
            &ctx(),
        );
        let dest = post(&plan, "work.md");
        assert!(dest.contains("- [ ] #task Existing ^a"), "{dest}");
        assert!(dest.contains("- [ ] #task Incoming"), "{dest}");
        assert!(dest.contains("^a-1"), "{dest}");
        assert_eq!(
            plan.outcomes[0]
                .target
                .as_ref()
                .and_then(|t| t.block_id.as_deref()),
            Some("a-1")
        );
    }

    #[test]
    fn id_less_locator_uses_the_checked_line() {
        let source = paired_source();
        let archive = paired_archive(
            "- [x] #task No id [[mail/aabbccdd]] [completion:: 2026-11-10] [fresh:: 2026-11-01]\n",
        );
        let linker = closed_task("done/work_done.md", &archive, "No id", true);
        assert!(linker.block_id.is_none());
        let plan = plan_reopens(
            snap(&[
                ("work.md", source.as_str()),
                ("done/work_done.md", archive.as_str()),
            ]),
            &[candidate(
                THREAD,
                MESSAGE,
                message_at(),
                "Ada",
                false,
                vec![linker],
            )],
            &ctx(),
        );
        assert_eq!(plan.outcomes[0].disposition, ReopenDisposition::Applied);
        assert!(post(&plan, "work.md").contains("- [ ] #task No id"));
    }

    #[test]
    fn repeated_message_is_a_logged_noop() {
        let task = "- [x] #task Closed [[mail/aabbccdd]] [completion:: 2026-11-10] [fresh:: 2026-11-01] ^t";
        let job = job_note(task);
        let linker = closed_task("job.md", &job, "Closed", false);
        let first = candidate(
            THREAD,
            MESSAGE,
            message_at(),
            "Ada",
            false,
            vec![linker.clone()],
        );
        let plan = plan_reopens(
            snap(&[("job.md", job.as_str())]),
            &[first.clone(), first],
            &ctx(),
        );
        assert_eq!(plan.outcomes[0].disposition, ReopenDisposition::Applied);
        assert_eq!(plan.outcomes[1].disposition, ReopenDisposition::NoOp);
        assert_eq!(plan.outcomes[1].reason, "already_logged");
        assert!(plan.outcomes[1].pending_marker);
        let dest = post(&plan, "job.md");
        assert_eq!(
            dest.matches(&format!("#^m-{MESSAGE}")).count(),
            1,
            "{dest}"
        );
    }

    #[test]
    fn two_threads_sharing_a_target_append_both_logs() {
        let task = "- [x] #task Shared [[mail/aabbccdd]] [completion:: 2026-11-10] [fresh:: 2026-11-01] ^t";
        let job = job_note(task);
        let linker = closed_task("job.md", &job, "Shared", false);
        let other = "bbccddee";
        let first = candidate(
            THREAD,
            MESSAGE,
            message_at(),
            "Ada",
            false,
            vec![linker.clone()],
        );
        let mut second = candidate(
            other,
            "2222bbbb",
            message_at(),
            "Bea",
            false,
            vec![linker],
        );
        second.evidence.note = format!("mail/{other}.md");
        let plan = plan_reopens(
            snap(&[("job.md", job.as_str())]),
            &[first, second],
            &ctx(),
        );
        assert_eq!(plan.outcomes[0].disposition, ReopenDisposition::Applied);
        assert_eq!(plan.outcomes[1].disposition, ReopenDisposition::Applied);
        let dest = post(&plan, "job.md");
        assert!(dest.contains("reply from Ada"), "{dest}");
        assert!(dest.contains("reply from Bea"), "{dest}");
        assert!(dest.contains(&format!("#^m-{MESSAGE}")), "{dest}");
        assert!(dest.contains("#^m-2222bbbb"), "{dest}");
    }

    #[test]
    fn preserves_crlf_and_a_missing_final_newline() {
        let task = "- [x] #task Closed [[mail/aabbccdd]] [completion:: 2026-11-10] [fresh:: 2026-11-01] ^t";
        let job = format!("## Tasks\r\n\r\n{task}");
        let linker = closed_task("job.md", &job, "Closed", false);
        let plan = plan_reopens(
            snap(&[("job.md", job.as_str())]),
            &[candidate(
                THREAD,
                MESSAGE,
                message_at(),
                "Ada",
                false,
                vec![linker],
            )],
            &ctx(),
        );
        let dest = post(&plan, "job.md");
        assert!(dest.contains("\r\n"), "{dest:?}");
        assert!(dest.contains("- [ ] #task Closed"), "{dest}");
        assert!(dest.contains("📬 **REOPEN LOG**"), "{dest}");
        assert!(!dest.ends_with('\n'), "{dest:?}");
    }

    #[test]
    fn prepends_to_existing_history_and_ignores_nested_or_fenced_markers() {
        let job = concat!(
            "## Tasks\n",
            "- [x] #task Closed [[mail/aabbccdd]] [completion:: 2026-11-10] [fresh:: 2026-11-01] ^t\n",
            "\t- note\n",
            "\t\t- 📬 **REOPEN LOG**\n",
            "\t\t\t- *2026-11-01* — nested lookalike\n",
            "  ```\n",
            "  - 📬 **REOPEN LOG**\n",
            "  ```\n",
            "\t- 📬 **REOPEN LOG**\n",
            "\t\t- *2026-11-12* — ✉️ reply from Old · [[aabbccdd#^m-old|2026-11-12 09:00]]\n",
        );
        let linker = closed_task("job.md", job, "Closed", false);
        let plan = plan_reopens(
            snap(&[("job.md", job)]),
            &[candidate(
                THREAD,
                MESSAGE,
                message_at(),
                "Ada",
                false,
                vec![linker],
            )],
            &ctx(),
        );
        let dest = post(&plan, "job.md");
        assert!(dest.contains("nested lookalike"), "{dest}");
        assert!(dest.contains("  - 📬 **REOPEN LOG**"), "{dest}");
        let old_entry =
            "*2026-11-12* — ✉️ reply from Old · [[aabbccdd#^m-old|2026-11-12 09:00]]";
        assert!(dest.contains(old_entry), "{dest}");
        let new_at = dest.find("reply from Ada").unwrap();
        let old_at = dest.find("reply from Old").unwrap();
        let nested_at = dest.find("nested lookalike").unwrap();
        let fenced_at = dest.find("  - 📬 **REOPEN LOG**").unwrap();
        assert!(nested_at < new_at, "{dest}");
        assert!(fenced_at < new_at, "{dest}");
        assert!(new_at < old_at, "{dest}");
        assert_eq!(
            dest.matches(&format!("#^m-{MESSAGE}")).count(),
            1,
            "{dest}"
        );
    }

    #[test]
    fn fenced_lookalike_with_the_same_card_does_not_count_as_logged() {
        let job = concat!(
            "## Tasks\n",
            "- [ ] #task Open [[mail/aabbccdd]] [fresh:: 2026-11-01] ^t\n",
            "\t- note\n",
            "  ```\n",
            "  - 📬 **REOPEN LOG**\n",
            "  - *2026-11-14* — ✉️ reply from Ada · [[aabbccdd#^m-1111aaaa|2026-11-14 10:00]]\n",
            "  ```\n",
        );
        let evidence_job = job_note(
            "- [x] #task Open [[mail/aabbccdd]] [completion:: 2026-11-10] [fresh:: 2026-11-01] ^t",
        );
        let linker = closed_task("job.md", &evidence_job, "Open", false);
        let plan = plan_reopens(
            snap(&[("job.md", job)]),
            &[candidate(
                THREAD,
                MESSAGE,
                message_at(),
                "Ada",
                false,
                vec![linker],
            )],
            &ctx(),
        );
        assert_eq!(plan.outcomes[0].disposition, ReopenDisposition::Applied);
        let dest = post(&plan, "job.md");
        assert!(dest.contains("\t- 📬 **REOPEN LOG**"), "{dest}");
        assert_eq!(
            dest.matches(&format!("#^m-{MESSAGE}")).count(),
            2,
            "{dest}"
        );
    }

    #[test]
    fn hostile_sender_stays_inert_and_bounce_uses_bounced() {
        let task = "- [x] #task Closed [[mail/aabbccdd]] [completion:: 2026-11-10] [fresh:: 2026-11-01] ^t";
        let job = job_note(task);
        let linker = closed_task("job.md", &job, "Closed", false);
        let sender = inert_single_line("Alice [[evil]] <!--boom-->");
        let plan = plan_reopens(
            snap(&[("job.md", job.as_str())]),
            &[candidate(
                THREAD,
                MESSAGE,
                message_at(),
                &sender,
                true,
                vec![linker],
            )],
            &ctx(),
        );
        let dest = post(&plan, "job.md");
        assert!(dest.contains("bounced"), "{dest}");
        assert!(!dest.contains("reply from"), "{dest}");
        assert!(!dest.contains("[[evil]]"), "{dest}");
        assert!(!dest.contains("<!--boom-->"), "{dest}");

        let reply = plan_reopens(
            snap(&[("job.md", job.as_str())]),
            &[candidate(
                THREAD,
                MESSAGE,
                message_at(),
                &sender,
                false,
                vec![closed_task("job.md", &job, "Closed", false)],
            )],
            &ctx(),
        );
        let dest = post(&reply, "job.md");
        assert!(dest.contains(&format!("reply from {sender}")), "{dest}");
        assert!(!dest.contains("[[evil]]"), "{dest}");
        assert!(!dest.contains("<!--boom-->"), "{dest}");
    }

    #[test]
    fn lifts_awaiting_hold_and_preserves_an_independent_schedule() {
        let held = "- [x] #task Held [[mail/aabbccdd]] #awaiting_reply [completion:: 2026-11-10] [fresh:: 2026-11-01] [scheduled:: 2026-11-20] ^t";
        let job = job_note(held);
        let linker = closed_task("job.md", &job, "Held", false);
        let plan = plan_reopens(
            snap(&[("job.md", job.as_str())]),
            &[candidate(
                THREAD,
                MESSAGE,
                message_at(),
                "Ada",
                false,
                vec![linker],
            )],
            &ctx(),
        );
        let dest = post(&plan, "job.md");
        assert!(!dest.contains(AWAITING_TAG), "{dest}");
        assert!(!dest.contains("[scheduled:: 2026-11-20]"), "{dest}");
        assert!(dest.contains("[unblocked:: 2026-11-14]"), "{dest}");

        let scheduled = "- [x] #task Dated [[mail/aabbccdd]] [completion:: 2026-11-10] [fresh:: 2026-11-01] [scheduled:: 2026-12-01] ^t";
        let job = job_note(scheduled);
        let linker = closed_task("job.md", &job, "Dated", false);
        let plan = plan_reopens(
            snap(&[("job.md", job.as_str())]),
            &[candidate(
                THREAD,
                MESSAGE,
                message_at(),
                "Ada",
                false,
                vec![linker],
            )],
            &ctx(),
        );
        let dest = post(&plan, "job.md");
        assert!(dest.contains("[scheduled:: 2026-12-01]"), "{dest}");
        assert!(dest.contains("- [?] #task Dated"), "{dest}");
        assert!(!dest.contains("[unblocked::"), "{dest}");
    }

    #[test]
    fn remaining_blockers_keep_blocked_and_skip_unblocked() {
        let deps = "- [x] #task Deps [[mail/aabbccdd]] [completion:: 2026-11-10] [fresh:: 2026-11-01] [dependsOn:: abc123] ^t";
        let job = job_note(deps);
        let linker = closed_task("job.md", &job, "Deps", false);
        let plan = plan_reopens(
            snap(&[("job.md", job.as_str())]),
            &[candidate(
                THREAD,
                MESSAGE,
                message_at(),
                "Ada",
                false,
                vec![linker],
            )],
            &ctx(),
        );
        let dest = post(&plan, "job.md");
        assert!(dest.contains("- [?] #task Deps"), "{dest}");
        assert!(dest.contains("[dependsOn:: abc123]"), "{dest}");
        assert!(!dest.contains("[unblocked::"), "{dest}");

        let prj = "- [x] #task Prj [[mail/aabbccdd]] #prj_is_blocked [completion:: 2026-11-10] [fresh:: 2026-11-01] ^t";
        let job = job_note(prj);
        let linker = closed_task("job.md", &job, "Prj", false);
        let plan = plan_reopens(
            snap(&[("job.md", job.as_str())]),
            &[candidate(
                THREAD,
                MESSAGE,
                message_at(),
                "Ada",
                false,
                vec![linker],
            )],
            &ctx(),
        );
        let dest = post(&plan, "job.md");
        assert!(dest.contains("- [?] #task Prj"), "{dest}");
        assert!(dest.contains("#prj_is_blocked"), "{dest}");
        assert!(!dest.contains("[unblocked::"), "{dest}");
    }

    #[test]
    fn invalid_blocked_config_leaves_ready_and_warns() {
        let deps = "- [x] #task Deps [[mail/aabbccdd]] [completion:: 2026-11-10] [fresh:: 2026-11-01] [dependsOn:: abc123] ^t";
        let job = job_note(deps);
        let linker = closed_task("job.md", &job, "Deps", false);
        let mut blocked = ctx();
        blocked.blocked_ok = false;
        blocked.blocked_reason =
            Some("Blocked status is not a single '?'".into());
        let plan = plan_reopens(
            snap(&[("job.md", job.as_str())]),
            &[candidate(
                THREAD,
                MESSAGE,
                message_at(),
                "Ada",
                false,
                vec![linker],
            )],
            &blocked,
        );
        let dest = post(&plan, "job.md");
        assert!(dest.contains("- [ ] #task Deps"), "{dest}");
        assert!(!dest.contains("[unblocked::"), "{dest}");
        assert!(plan.outcomes[0]
            .warnings
            .iter()
            .any(|warning| warning.contains("Blocked status")));
    }

    #[test]
    fn new_and_today_write_skips_omit_unblocked() {
        let new_row = "- [x] #task New [[mail/aabbccdd]] [completion:: 2026-11-10] [created:: 2026-11-01] ^t";
        let job = job_note(new_row);
        let linker = closed_task("job.md", &job, "New", false);
        let plan = plan_reopens(
            snap(&[("job.md", job.as_str())]),
            &[candidate(
                THREAD,
                MESSAGE,
                message_at(),
                "Ada",
                false,
                vec![linker],
            )],
            &ctx(),
        );
        let dest = post(&plan, "job.md");
        assert!(dest.contains("- [ ] #task New"), "{dest}");
        assert!(!dest.contains("[unblocked::"), "{dest}");

        let today_row = "- [x] #task Today [[mail/aabbccdd]] [completion:: 2026-11-10] [fresh:: 2026-11-01] ^t";
        let job = job_note(today_row);
        let linker = closed_task("job.md", &job, "Today", false);
        let mut today_ctx = ctx();
        today_ctx
            .today_identities
            .insert(("job.md".into(), "t".into()));
        let plan = plan_reopens(
            snap(&[("job.md", job.as_str())]),
            &[candidate(
                THREAD,
                MESSAGE,
                message_at(),
                "Ada",
                false,
                vec![linker],
            )],
            &today_ctx,
        );
        assert!(!post(&plan, "job.md").contains("[unblocked::"));
    }

    #[test]
    fn recurring_and_project_rows_are_permanent_refusals() {
        let recurring = "- [x] #task Repeat [[mail/aabbccdd]] [repeat:: every weekday] [completion:: 2026-11-10] ^t";
        let job = job_note(recurring);
        let linker = closed_task("job.md", &job, "Repeat", false);
        let plan = plan_reopens(
            snap(&[("job.md", job.as_str())]),
            &[candidate(
                THREAD,
                MESSAGE,
                message_at(),
                "Ada",
                false,
                vec![linker],
            )],
            &ctx(),
        );
        assert_eq!(
            plan.outcomes[0].disposition,
            ReopenDisposition::PermanentRefusal
        );
        assert_eq!(plan.outcomes[0].reason, "recurring");
        assert_eq!(post(&plan, "job.md"), job);

        let prj = "- [x] #task Project [[mail/aabbccdd]] [completion:: 2026-11-10] ^prj";
        let job = job_note(prj);
        let linker = closed_task("job.md", &job, "Project", false);
        let plan = plan_reopens(
            snap(&[("job.md", job.as_str())]),
            &[candidate(
                THREAD,
                MESSAGE,
                message_at(),
                "Ada",
                false,
                vec![linker],
            )],
            &ctx(),
        );
        assert_eq!(
            plan.outcomes[0].disposition,
            ReopenDisposition::PermanentRefusal
        );
        assert_eq!(plan.outcomes[0].reason, "project_task");
        assert_eq!(post(&plan, "job.md"), job);
    }

    #[test]
    fn stale_or_missing_lines_defer() {
        let task =
            "- [x] #task Closed [[mail/aabbccdd]] [completion:: 2026-11-10] ^t";
        let job = job_note(task);
        let mut linker = closed_task("job.md", &job, "Closed", false);
        linker.line = 99;
        linker.block_id = None;
        let plan = plan_reopens(
            snap(&[("job.md", job.as_str())]),
            &[candidate(
                THREAD,
                MESSAGE,
                message_at(),
                "Ada",
                false,
                vec![linker],
            )],
            &ctx(),
        );
        assert_eq!(
            plan.outcomes[0].disposition,
            ReopenDisposition::TransientDeferred
        );
        assert_eq!(plan.outcomes[0].reason, "stale_line");

        let linker = closed_task("job.md", &job, "Closed", false);
        let plan = plan_reopens(
            snap(&[("other.md", "hi\n")]),
            &[candidate(
                THREAD,
                MESSAGE,
                message_at(),
                "Ada",
                false,
                vec![linker],
            )],
            &ctx(),
        );
        assert_eq!(
            plan.outcomes[0].disposition,
            ReopenDisposition::TransientDeferred
        );
        assert_eq!(plan.outcomes[0].reason, "missing_note");
    }

    #[test]
    fn overlay_keeps_mail_saves_while_editing_the_task() {
        let task = "- [x] #task Closed [[mail/aabbccdd]] [completion:: 2026-11-10] [fresh:: 2026-11-01] ^t";
        let job = job_note(task);
        let mail = "---\ntype: \"[[mail]]\"\n---\n# Saved mail\n";
        let linker = closed_task("job.md", &job, "Closed", false);
        let plan = plan_reopens(
            snap(&[("job.md", job.as_str()), ("mail/aabbccdd.md", mail)]),
            &[candidate(
                THREAD,
                MESSAGE,
                message_at(),
                "Ada",
                false,
                vec![linker],
            )],
            &ctx(),
        );
        assert_eq!(post(&plan, "mail/aabbccdd.md"), mail);
        assert!(post(&plan, "job.md").contains("📬 **REOPEN LOG**"));
    }

    #[test]
    fn no_close_date_skips_the_date_guard() {
        let task =
            "- [x] #task Closed [[mail/aabbccdd]] [fresh:: 2026-11-01] ^t";
        let job = job_note(task);
        let linker = closed_task("job.md", &job, "Closed", false);
        let plan = plan_reopens(
            snap(&[("job.md", job.as_str())]),
            &[candidate(
                THREAD,
                MESSAGE,
                at("2020-01-01T00:00:00Z"),
                "Ada",
                false,
                vec![linker],
            )],
            &ctx(),
        );
        assert_eq!(plan.outcomes[0].disposition, ReopenDisposition::Applied);
    }

    #[test]
    fn newest_recurring_does_not_fall_back_to_an_older_task() {
        let recurring = "- [x] #task Repeat [[mail/aabbccdd]] [repeat:: every weekday] [completion:: 2026-11-12] ^new";
        let older = "- [x] #task Older [[mail/aabbccdd]] [completion:: 2026-11-01] [fresh:: 2026-10-01] ^old";
        let job = format!("## Tasks\n\n{older}\n{recurring}\n");
        let tasks = vec![
            closed_task("job.md", &job, "Older", false),
            closed_task("job.md", &job, "Repeat", false),
        ];
        let plan = plan_reopens(
            snap(&[("job.md", job.as_str())]),
            &[candidate(
                THREAD,
                MESSAGE,
                message_at(),
                "Ada",
                false,
                tasks,
            )],
            &ctx(),
        );
        assert_eq!(
            plan.outcomes[0].disposition,
            ReopenDisposition::PermanentRefusal
        );
        assert_eq!(plan.outcomes[0].reason, "recurring");
        assert_eq!(post(&plan, "job.md"), job);
    }
}

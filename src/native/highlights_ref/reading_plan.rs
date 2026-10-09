//! Pure v2 reading-task planning (phase `v2-planning`).
//!
//! Index-aware but side-effect-free: classifies the v1/v2/birth/reopen branch
//! for one PDF, resolves birth and reopen parents through an injected
//! resolver, compares the located task status against the parent-free stored
//! base, and renders or heals the managed embed. Nothing here touches the
//! vault or the PDF marker; the scan entrypoints call [`plan_reading_task`]
//! directly, and the `v2-execution` worker consumes [`ReadingTaskPlan`] for
//! the guarded writes.

use super::*;
use crate::native::parent_notes::{ParentError, ResolvedParent};
use crate::native::ref_tasks::{
    find_managed_embed, is_open_mark, managed_embed_line, select_for_ref,
    LocatedRefTask, RefTaskDiagnostic, Selected, TrackerHit,
};

/// Which sync branch owns one PDF's note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NoteBranch {
    /// Today's path: in-note `^ref` trackers and trackerless legacy notes.
    /// Behavior stays byte-for-byte.
    V1,
    /// The reading task lives (or will live) outside the ref note; residence,
    /// embed, and cross-file status rules apply.
    V2,
}

/// Classify one note: a missing note is a v2 birth; an existing note is v2
/// when the locator found its task outside the note (live or archived) or
/// the body already carries the managed embed. Everything else — in-note
/// `^ref` trackers and trackerless legacy notes — stays on the v1 branch.
///
/// `ref_note_rel` is the note's vault-relative path with `.md` (the same
/// form as [`LocatedRefTask::path`]).
pub(super) fn classify_note_branch(
    note_exists: bool,
    ref_note_rel: &str,
    body: &str,
    candidates: &[LocatedRefTask],
) -> NoteBranch {
    if !note_exists {
        return NoteBranch::V2;
    }
    if find_managed_embed(body).is_some() {
        return NoteBranch::V2;
    }
    if candidates
        .iter()
        .any(|candidate| candidate.path != ref_note_rel)
    {
        return NoteBranch::V2;
    }
    NoteBranch::V1
}

/// Fallback parent route when the marker hint cannot be resolved.
pub(super) const FALLBACK_PARENT_ROUTE: &str = "mac_inbox";

/// Child bullet filed with a fallback birth when the marker's parent hint is
/// not an open area or project. Keeps the original hint verbatim.
pub(super) fn parent_fallback_child(hint: &str) -> String {
    format!(
        "⚠️ parent '{hint}' is not an open area or project · refile me with Ctrl+Shift+M"
    )
}

/// Where a birth or reopen line goes: the canonical route, its label, an
/// optional warning child (fallback only), and the original hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BirthParent {
    pub route: String,
    pub label: String,
    pub warning_child: Option<String>,
    pub hint: String,
}

/// Resolve a birth parent through the shared resolver. Any failure —
/// unknown, ambiguous, terminal, or non-parent — falls back to `mac_inbox`
/// with the warning child; the caller never guesses a residence.
///
/// The resolver is injected so tests stay vault-free; the scan entrypoint
/// passes `|hint| resolve_parent(&config.bob_dir, hint)`.
pub(super) fn resolve_birth_parent(
    hint: &str,
    resolver: &dyn Fn(&str) -> std::result::Result<ResolvedParent, ParentError>,
) -> BirthParent {
    match resolver(hint) {
        Ok(resolved) => BirthParent {
            route: resolved.route,
            label: resolved.label,
            warning_child: None,
            hint: hint.to_string(),
        },
        Err(_) => BirthParent {
            route: FALLBACK_PARENT_ROUTE.to_string(),
            label: format!("{FALLBACK_PARENT_ROUTE}.md"),
            warning_child: Some(parent_fallback_child(hint)),
            hint: hint.to_string(),
        },
    }
}

/// What a birth does about the reading-task line itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BirthTask {
    /// A uniquely located orphan is adopted: only the ref note is written.
    Adopt(LocatedRefTask),
    /// No orphan exists: insert a fresh line into the birth parent.
    Insert,
    /// Several claimants (or another unselectable state): refuse to write
    /// rather than duplicate; diagnostics explain.
    Refused,
}

/// Pick the birth task from this ref's located candidates, reusing the
/// locator's selection rules: a unique selection (open or newest closed) is
/// adopted, no candidates means a fresh insert, and anything unselectable is
/// refused. Returns the locator diagnostics alongside for the plan report.
pub(super) fn select_birth_task(
    candidates: &[LocatedRefTask],
) -> (BirthTask, Vec<RefTaskDiagnostic>) {
    let selection = select_for_ref(candidates, &[]);
    let task = match selection.task {
        Some(Selected::V2(located)) => BirthTask::Adopt(located),
        // Unreachable today (`select_for_ref` never returns V1 with no
        // hits); refuse rather than invent a task.
        Some(Selected::V1(_)) => BirthTask::Refused,
        None if candidates.is_empty() => BirthTask::Insert,
        None => BirthTask::Refused,
    };
    (task, selection.diagnostics)
}

/// Guidance for an open v2 note whose task is gone everywhere: never an
/// automatic replacement. Terminal notes stay silent.
pub(super) fn missing_task_diagnostic(
    resolved_status: Option<&str>,
) -> Option<RefTaskDiagnostic> {
    if matches!(resolved_status, Some(STATUS_READ | STATUS_ABANDONED)) {
        return None;
    }
    Some(RefTaskDiagnostic::new(
        "open_ref_without_task",
        "open v2 note has no reading task; restore it from git or set status: abandoned".to_string(),
    ))
}

/// Checkbox mark for a synced status, mirroring
/// `projection_pdf_task_mark`'s mapping. `None` for statuses no checkbox
/// expresses (`legacy`, missing, unknown).
pub(super) fn status_mark(status: Option<&str>) -> Option<char> {
    match status {
        Some(STATUS_READY) => Some(' '),
        Some(STATUS_NEXT) => Some('*'),
        Some(STATUS_WIP) => Some('/'),
        Some(STATUS_READ) => Some('x'),
        Some(STATUS_ABANDONED) => Some('-'),
        _ => None,
    }
}

/// Inputs to the v2 task-status decision. All projections are parent-free
/// (see `without_parent`); statuses are their `status` values.
pub(super) struct V2TaskSignalInputs<'a> {
    pub mark: char,
    pub current_status: Option<&'a str>,
    pub base_status: Option<&'a str>,
    pub marker_status: Option<&'a str>,
    pub frontmatter_status: Option<&'a str>,
    pub task_archived: bool,
}

/// The v2 task-status decision: pure values for the executor to apply.
/// `drive_status` flows into the note frontmatter (and the PDF marker only
/// under `--write-pdf(s)`); `checkbox_edit` is a one-line edit on the
/// located line; `reopen` inserts a fresh line and never mutates `done/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct V2TaskSignal {
    pub task_status: PdfTaskStatus,
    pub target: Option<&'static str>,
    pub task_changed: bool,
    pub drive_status: Option<&'static str>,
    pub checkbox_edit: Option<char>,
    pub reopen: bool,
}

fn quiet_signal(
    task_status: PdfTaskStatus,
    target: Option<&'static str>,
    task_changed: bool,
) -> V2TaskSignal {
    V2TaskSignal {
        task_status,
        target,
        task_changed,
        drive_status: None,
        checkbox_edit: None,
        reopen: false,
    }
}

/// Feed the located mark into the status policy, `[?]` included as an
/// open-lane overlay via `ref_task_mark_target_status`. The v1 signal
/// (`apply_pdf_task_status_signal`) is untouched; this is the v2 branch:
///
/// - The task status is compared against the stored base to tell an
///   unchanged checkbox from a new task gesture.
/// - A task gesture alone drives the synced status (like v1).
/// - Marker/frontmatter-only changes drive a checkbox edit instead of
///   conflicting.
/// - Incompatible changed inputs still conflict, as does a disagreement
///   with no stored base to reconcile it.
/// - A deliberate open signal against a terminal task reopens: a fresh
///   insert for archived tasks (scan never mutates `done/`), a checkbox
///   edit for live ones. The old terminal mark never overrides the reopen.
pub(super) fn v2_task_status_signal(
    inputs: V2TaskSignalInputs<'_>,
) -> Result<V2TaskSignal> {
    let task_status = match inputs.mark {
        ' ' => PdfTaskStatus::Ready,
        '*' => PdfTaskStatus::Next,
        '/' => PdfTaskStatus::Wip,
        '?' => PdfTaskStatus::Blocked,
        'x' | 'X' => PdfTaskStatus::Read,
        '-' => PdfTaskStatus::Abandoned,
        _ => PdfTaskStatus::Missing,
    };
    let target =
        ref_task_mark_target_status(inputs.mark, inputs.current_status);
    let Some(target) = target else {
        return Ok(quiet_signal(task_status, None, false));
    };
    let task_changed = inputs.base_status.is_some_and(|base| base != target);
    if Some(target) == inputs.current_status {
        return Ok(quiet_signal(task_status, Some(target), task_changed));
    }
    if task_changed {
        let mut conflicts = Vec::new();
        for (source, value) in [
            ("marker", inputs.marker_status),
            ("frontmatter", inputs.frontmatter_status),
        ] {
            if value != inputs.base_status && value != Some(target) {
                conflicts.push(PdfTaskStatusConflict {
                    source,
                    base: inputs
                        .base_status
                        .map(|status| MarkerValue::String(status.to_string())),
                    value: value
                        .map(|status| MarkerValue::String(status.to_string())),
                });
            }
        }
        if !conflicts.is_empty() {
            return Err(pdf_task_status_conflict_error(
                task_status,
                target,
                &conflicts,
            ));
        }
        return Ok(V2TaskSignal {
            task_status,
            target: Some(target),
            task_changed: true,
            drive_status: Some(target),
            checkbox_edit: None,
            reopen: false,
        });
    }
    let Some(_base) = inputs.base_status else {
        return Err(CommandError::new(format!(
            "{} reading task disagrees with the synced status with no stored base to reconcile them: task [{}] wants {target}, note says {}; edit the reading task or the note status so they agree, then rerun",
            task_status.label(),
            inputs.mark,
            inputs.current_status.unwrap_or("<missing>"),
        )));
    };
    // The checkbox is unchanged: a marker/frontmatter-only change. A
    // deliberate open signal against a terminal task is a reopen.
    let task_terminal = matches!(inputs.mark, 'x' | 'X' | '-');
    let current_open =
        !matches!(inputs.current_status, Some(STATUS_READ | STATUS_ABANDONED));
    if task_terminal && current_open {
        if inputs.task_archived {
            return Ok(V2TaskSignal {
                task_status,
                target: Some(target),
                task_changed: false,
                drive_status: None,
                checkbox_edit: None,
                reopen: true,
            });
        }
        let Some(edit) = status_mark(inputs.current_status) else {
            return Err(CommandError::new(format!(
                "{} reading task cannot reopen to status {}; edit the reading task or the note status so they agree, then rerun",
                task_status.label(),
                inputs.current_status.unwrap_or("<missing>"),
            )));
        };
        return Ok(V2TaskSignal {
            task_status,
            target: Some(target),
            task_changed: false,
            drive_status: None,
            checkbox_edit: Some(edit),
            reopen: false,
        });
    }
    let Some(edit) = status_mark(inputs.current_status) else {
        return Err(CommandError::new(format!(
            "{} reading task disagrees with marker/frontmatter status {}; edit the reading task or the note status so they agree, then rerun",
            task_status.label(),
            inputs.current_status.unwrap_or("<missing>"),
        )));
    };
    Ok(V2TaskSignal {
        task_status,
        target: Some(target),
        task_changed: false,
        drive_status: None,
        checkbox_edit: Some(edit),
        reopen: false,
    })
}

/// Destination for a deliberate archive reopen: the archive's source parent
/// when it still accepts work, else the inbox fallback with the warning
/// child. `source_route` is the archived residence (already a route);
/// `source_open` comes from the caller checking `parent_candidates`.
pub(super) fn plan_reopen_destination(
    source_route: Option<&str>,
    source_open: bool,
) -> BirthParent {
    match source_route {
        Some(route) if source_open => BirthParent {
            route: route.to_string(),
            label: format!("{route}.md"),
            warning_child: None,
            hint: route.to_string(),
        },
        _ => {
            let hint = source_route.unwrap_or("unknown");
            BirthParent {
                route: FALLBACK_PARENT_ROUTE.to_string(),
                label: format!("{FALLBACK_PARENT_ROUTE}.md"),
                warning_child: Some(parent_fallback_child(hint)),
                hint: hint.to_string(),
            }
        }
    }
}

/// Managed-embed target for a located task: the route for a root note
/// (`sase`) and the vault-relative path without `.md` otherwise
/// (`done/sase_done`). Embeds are views, never identity.
pub(super) fn managed_embed_target(task: &LocatedRefTask) -> String {
    task.path
        .strip_suffix(".md")
        .or_else(|| task.path.strip_suffix(".MD"))
        .unwrap_or(&task.path)
        .to_string()
}

/// Split a body into lines, the dominant line ending, and whether the body
/// ends with a newline. Shared by embed healing and audio anchoring so both
/// preserve CRLF bodies byte-for-byte outside their slot.
pub(super) fn split_body_lines(
    body: &str,
) -> (Vec<String>, &'static str, bool) {
    let ending = if body.contains("\r\n") { "\r\n" } else { "\n" };
    let trailing = body.ends_with('\n');
    (body.lines().map(str::to_string).collect(), ending, trailing)
}

/// Rejoin lines split by [`split_body_lines`].
pub(super) fn join_body_lines(
    lines: &[String],
    ending: &str,
    trailing: bool,
) -> String {
    let mut rendered = lines.join(ending);
    if trailing {
        rendered.push_str(ending);
    }
    rendered
}

fn first_h1_line_index(lines: &[String]) -> Option<usize> {
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let fenced = markdown::fenced_lines(&refs, 0..refs.len());
    lines.iter().enumerate().find_map(|(index, line)| {
        (!fenced.contains(&index)
            && matches!(markdown::atx_heading(line), Some((1, _))))
        .then_some(index)
    })
}

/// Heal the managed embed slot to exactly one
/// `![[<target>#^<id>]]` line, one blank line below the H1. Removes every
/// managed-anatomy embed in the slot (missing, stale, or duplicated) and
/// reinserts the canonical one; authored material around the slot —
/// including unrelated embeds elsewhere — is preserved byte-for-byte, as
/// are line endings. Bodies without an H1 are returned unchanged: with no
/// slot to heal into, no target is invented.
pub(super) fn heal_managed_embed(
    body: &str,
    embed_target: &str,
    block_id: &str,
) -> String {
    let expected = managed_embed_line(embed_target, block_id);
    let (probe, _, _) = split_body_lines(body);
    if first_h1_line_index(&probe).is_none() {
        return body.to_string();
    }
    let mut current = body.to_string();
    while let Some(found) = find_managed_embed(&current) {
        let (mut lines, ending, trailing) = split_body_lines(&current);
        if found.line_index >= lines.len() {
            break;
        }
        lines.remove(found.line_index);
        current = join_body_lines(&lines, ending, trailing);
    }
    let (mut lines, ending, trailing) = split_body_lines(&current);
    let Some(h1) = first_h1_line_index(&lines) else {
        return current;
    };
    if lines.get(h1 + 1).is_none_or(|line| !line.trim().is_empty()) {
        lines.insert(h1 + 1, String::new());
    }
    if lines.get(h1 + 2).is_none_or(|line| line.trim() != expected) {
        lines.insert(h1 + 2, expected);
    }
    if lines.get(h1 + 3).is_none_or(|line| !line.trim().is_empty()) {
        lines.insert(h1 + 3, String::new());
    }
    join_body_lines(&lines, ending, trailing)
}

/// Render a v2 birth body: the H1, the managed embed where the tracker sat,
/// the companion audio (when any), then the Highlights region. No in-note
/// `^ref` tracker, no `#hide`. `highlights` is the managed-region content
/// (possibly empty); the executor supplies the allocated block ID.
pub(super) fn v2_birth_body(
    title: &str,
    embed_target: &str,
    block_id: &str,
    highlights: &str,
    audio: Option<&str>,
) -> String {
    let mut body = String::from("\n# ");
    body.push_str(title);
    body.push_str("\n\n");
    body.push_str(&managed_embed_line(embed_target, block_id));
    body.push_str("\n\n");
    if let Some(audio) = audio {
        body.push_str(&audio_embed_line(audio));
        body.push_str("\n\n");
    }
    body.push_str("## Highlights\n\n");
    body.push_str(MANAGED_BODY_BEGIN);
    body.push_str("\n\n");
    body.push_str(highlights);
    if !highlights.is_empty() && !highlights.ends_with('\n') {
        body.push('\n');
    }
    body.push_str(MANAGED_BODY_END);
    body.push('\n');
    body
}

/// What one PDF's reading task means for the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ReadingTaskKind {
    /// Today's path; the executor leaves planning to `plan_pdf_sync`.
    V1,
    /// Missing note: adopt an orphan or insert into the birth parent.
    Birth,
    /// A live located task drives sync; edits go to its line.
    Existing,
    /// A closed task supplies terminal state; nothing is written.
    ClosedTerminal,
    /// A deliberate open signal against an archived-terminal task inserts
    /// a fresh line into the archive's source parent (or inbox fallback).
    Reopen,
    /// An open v2 note with no task anywhere: guidance only, no writes.
    Missing,
    /// Ambiguity or unselectable state: refuse status/parent/task writes.
    Refused,
}

/// The one destination write a reading-task plan asks for, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ReadingTaskAction {
    /// No destination write.
    NoWrite,
    /// Insert a fresh line. `prefer_block_id` reuses the old ID when free
    /// (reopen keeps the user-renamed address); `None` allocates.
    Insert {
        destination_route: String,
        destination_label: String,
        warning_child: Option<String>,
        mark: char,
        prefer_block_id: Option<String>,
    },
    /// Flip the checkbox on the located line (the executor re-reads and
    /// stamps closes; archived lines never take this action).
    LineEdit { target_mark: char },
}

/// Where the ref note's embed points after this plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReadingTaskEmbed {
    pub target: String,
    pub block_id: String,
}

/// The pure reading-task plan for one PDF: the executor's whole input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReadingTaskPlan {
    pub branch: NoteBranch,
    pub kind: ReadingTaskKind,
    pub action: ReadingTaskAction,
    pub residence: Option<String>,
    pub embed: Option<ReadingTaskEmbed>,
    pub status_target: Option<&'static str>,
    pub task_changed: bool,
    pub refuse_status_parent_writes: bool,
    pub diagnostics: Vec<RefTaskDiagnostic>,
}

/// Pure inputs to [`plan_reading_task`]. Statuses are the parent-free
/// `status` values; `archive_source_open` tells whether the archived
/// residence's source parent still accepts work.
pub(super) struct ReadingTaskPlanInputs<'a> {
    pub note_exists: bool,
    pub ref_note_rel: &'a str,
    pub note_body: &'a str,
    pub candidates: &'a [LocatedRefTask],
    pub v1_hits: &'a [TrackerHit],
    pub parent_hint: &'a str,
    pub resolved_status: Option<&'a str>,
    pub base_status: Option<&'a str>,
    pub marker_status: Option<&'a str>,
    pub frontmatter_status: Option<&'a str>,
    pub archive_source_open: bool,
    pub resolver:
        &'a dyn Fn(&str) -> std::result::Result<ResolvedParent, ParentError>,
}

fn embed_for_task(task: &LocatedRefTask) -> Option<ReadingTaskEmbed> {
    task.block_id.clone().map(|block_id| ReadingTaskEmbed {
        target: managed_embed_target(task),
        block_id,
    })
}

/// The migrate-tasks hint for one note's in-note trackers: `Some` when an
/// open `^ref` tracker remains, whatever branch owns the note. Shared by the
/// v2 planner and the legacy v1 scan path so both count the same remainder.
pub(super) fn open_v1_tracker_diagnostic(
    v1_hits: &[TrackerHit],
) -> Option<RefTaskDiagnostic> {
    v1_hits.iter().any(|hit| is_open_mark(hit.mark)).then(|| {
        RefTaskDiagnostic::new(
            "open_v1_tracker",
            "open in-note ^ref tracker; run bob ref migrate-tasks to move it into its parent note".to_string(),
        )
    })
}

/// Compose the full reading-task plan for one PDF: classify, then plan the
/// birth or the existing-task sync. V1 notes return a hands-off plan; the
/// executor keeps today's path for them exactly.
pub(super) fn plan_reading_task(
    inputs: ReadingTaskPlanInputs<'_>,
) -> Result<ReadingTaskPlan> {
    let branch = classify_note_branch(
        inputs.note_exists,
        inputs.ref_note_rel,
        inputs.note_body,
        inputs.candidates,
    );
    if branch == NoteBranch::V1 {
        // A v1 note keeps today's hands-off plan, but an open in-note
        // tracker is still reported so scan summaries can count the
        // remaining unmigrated references. Bytes and writes are unchanged.
        return Ok(ReadingTaskPlan {
            diagnostics: open_v1_tracker_diagnostic(inputs.v1_hits)
                .into_iter()
                .collect(),
            branch,
            kind: ReadingTaskKind::V1,
            action: ReadingTaskAction::NoWrite,
            residence: None,
            embed: None,
            status_target: None,
            task_changed: false,
            refuse_status_parent_writes: false,
        });
    }
    if !inputs.note_exists {
        return Ok(plan_birth_task(&inputs));
    }
    plan_existing_task(&inputs)
}

fn plan_birth_task(inputs: &ReadingTaskPlanInputs<'_>) -> ReadingTaskPlan {
    let parent = resolve_birth_parent(inputs.parent_hint, inputs.resolver);
    let (task, diagnostics) = select_birth_task(inputs.candidates);
    match task {
        BirthTask::Adopt(located) => ReadingTaskPlan {
            branch: NoteBranch::V2,
            kind: ReadingTaskKind::Birth,
            action: ReadingTaskAction::NoWrite,
            residence: located.residence.clone(),
            embed: embed_for_task(&located),
            status_target: None,
            task_changed: false,
            refuse_status_parent_writes: false,
            diagnostics,
        },
        BirthTask::Insert => ReadingTaskPlan {
            branch: NoteBranch::V2,
            kind: ReadingTaskKind::Birth,
            action: ReadingTaskAction::Insert {
                destination_route: parent.route,
                destination_label: parent.label,
                warning_child: parent.warning_child,
                mark: status_mark(inputs.resolved_status).unwrap_or(' '),
                prefer_block_id: None,
            },
            residence: None,
            // The block ID is allocated at execution against fresh bytes;
            // the executor renders the birth body then.
            embed: None,
            status_target: None,
            task_changed: false,
            refuse_status_parent_writes: false,
            diagnostics,
        },
        BirthTask::Refused => ReadingTaskPlan {
            branch: NoteBranch::V2,
            kind: ReadingTaskKind::Refused,
            action: ReadingTaskAction::NoWrite,
            residence: None,
            embed: None,
            status_target: None,
            task_changed: false,
            refuse_status_parent_writes: true,
            diagnostics,
        },
    }
}

fn plan_existing_task(
    inputs: &ReadingTaskPlanInputs<'_>,
) -> Result<ReadingTaskPlan> {
    let selection = select_for_ref(inputs.candidates, inputs.v1_hits);
    let Some(selected) = selection.task else {
        return Ok(unselected_task_plan(inputs, &selection.diagnostics));
    };
    let Selected::V2(located) = selected else {
        return Ok(ReadingTaskPlan {
            branch: NoteBranch::V2,
            kind: ReadingTaskKind::Refused,
            action: ReadingTaskAction::NoWrite,
            residence: None,
            embed: None,
            status_target: None,
            task_changed: false,
            refuse_status_parent_writes: true,
            diagnostics: selection.diagnostics,
        });
    };
    let signal = v2_task_status_signal(V2TaskSignalInputs {
        mark: located.mark,
        current_status: inputs.resolved_status,
        base_status: inputs.base_status,
        marker_status: inputs.marker_status,
        frontmatter_status: inputs.frontmatter_status,
        task_archived: located.archived,
    })?;
    let residence = located.residence.clone();
    let embed = embed_for_task(&located);
    if signal.reopen {
        let parent = plan_reopen_destination(
            located.residence.as_deref(),
            inputs.archive_source_open,
        );
        // A reopen never reuses the archived task's old residence: the
        // projected/destination residence, follow-ups, frontmatter parent,
        // and embed all use the new insert route (source parent or inbox
        // fallback). The archive and any terminal source stay unchanged.
        let reopen_embed =
            located.block_id.clone().map(|block_id| ReadingTaskEmbed {
                target: parent.route.clone(),
                block_id,
            });
        return Ok(ReadingTaskPlan {
            branch: NoteBranch::V2,
            kind: ReadingTaskKind::Reopen,
            action: ReadingTaskAction::Insert {
                destination_route: parent.route.clone(),
                destination_label: parent.label,
                warning_child: parent.warning_child,
                mark: status_mark(inputs.resolved_status).unwrap_or(' '),
                prefer_block_id: located.block_id.clone(),
            },
            residence: Some(parent.route),
            embed: reopen_embed,
            status_target: signal.target,
            task_changed: signal.task_changed,
            refuse_status_parent_writes: false,
            diagnostics: selection.diagnostics,
        });
    }
    if let Some(target_mark) = signal.checkbox_edit {
        return Ok(ReadingTaskPlan {
            branch: NoteBranch::V2,
            kind: if matches!(located.mark, 'x' | 'X' | '-') {
                ReadingTaskKind::ClosedTerminal
            } else {
                ReadingTaskKind::Existing
            },
            action: ReadingTaskAction::LineEdit { target_mark },
            residence,
            embed,
            status_target: signal.target,
            task_changed: signal.task_changed,
            refuse_status_parent_writes: false,
            diagnostics: selection.diagnostics,
        });
    }
    Ok(ReadingTaskPlan {
        branch: NoteBranch::V2,
        kind: if matches!(located.mark, 'x' | 'X' | '-') {
            ReadingTaskKind::ClosedTerminal
        } else {
            ReadingTaskKind::Existing
        },
        action: ReadingTaskAction::NoWrite,
        residence,
        embed,
        status_target: signal.drive_status,
        task_changed: signal.task_changed,
        refuse_status_parent_writes: false,
        diagnostics: selection.diagnostics,
    })
}

/// No selectable task on an existing v2 note: a missing-task plan when
/// nothing claims the ref, a refusal when claimants exist but none can be
/// used. Never invents a residence, block ID, or replacement task.
fn unselected_task_plan(
    inputs: &ReadingTaskPlanInputs<'_>,
    diagnostics: &[RefTaskDiagnostic],
) -> ReadingTaskPlan {
    if inputs.candidates.is_empty() {
        let mut diagnostics = diagnostics.to_vec();
        diagnostics.extend(missing_task_diagnostic(inputs.resolved_status));
        return ReadingTaskPlan {
            branch: NoteBranch::V2,
            kind: ReadingTaskKind::Missing,
            action: ReadingTaskAction::NoWrite,
            residence: None,
            embed: None,
            status_target: None,
            task_changed: false,
            refuse_status_parent_writes: false,
            diagnostics,
        };
    }
    ReadingTaskPlan {
        branch: NoteBranch::V2,
        kind: ReadingTaskKind::Refused,
        action: ReadingTaskAction::NoWrite,
        residence: None,
        embed: None,
        status_target: None,
        task_changed: false,
        refuse_status_parent_writes: true,
        diagnostics: diagnostics.to_vec(),
    }
}

/// Apply the v2 migration edits to one ref note's contents.
///
/// Removes the open v1 tracker block, heals the managed embed slot to
/// `![[<route>#^<block_id>]]` one blank line below the H1, sets
/// frontmatter `parent` to `parent: "[[<route>]]"`, and recomputes
/// `highlights_marker_hash`/`highlights_marker_base` from the
/// parent-free projection when those lines exist. The rest of the note
/// is preserved byte-for-byte.
pub(crate) fn apply_v2_migration_note(
    contents: &str,
    route: &str,
    block_id: &str,
) -> std::result::Result<String, String> {
    use crate::native::ref_tasks::managed_embed_line;
    let (front_opt, body) = match super::split_frontmatter(contents) {
        Some((front, body)) => (Some(front), body),
        None => (None, contents.to_string()),
    };
    // Remove the first open v1 tracker block from the body.
    let lines: Vec<&str> = body.lines().collect();
    let mut tracker_idx: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        if let Some(hit) = crate::native::ref_tasks::parse_tracker_line(line) {
            if crate::native::ref_tasks::is_open_mark(hit.mark) {
                tracker_idx = Some(i);
                break;
            }
        }
    }
    let mut body_lines: Vec<String> =
        lines.iter().map(|s| s.to_string()).collect();
    if let Some(idx) = tracker_idx {
        let indent = body_lines[idx]
            .find(|c: char| !c.is_whitespace())
            .unwrap_or(0);
        let mut end = idx + 1;
        let mut i = idx + 1;
        while i < body_lines.len() {
            if body_lines[i].trim().is_empty() {
                let next = body_lines[i + 1..]
                    .iter()
                    .position(|l| !l.trim().is_empty())
                    .map(|o| i + 1 + o);
                if next.is_some_and(|n| {
                    body_lines[n]
                        .find(|c: char| !c.is_whitespace())
                        .unwrap_or(0)
                        > indent
                }) {
                    end = i + 1;
                    i += 1;
                    continue;
                }
                break;
            }
            let ind = body_lines[i]
                .find(|c: char| !c.is_whitespace())
                .unwrap_or(body_lines[i].len());
            if ind <= indent {
                break;
            }
            end = i + 1;
            i += 1;
        }
        body_lines.drain(idx..end);
    }
    let pruned = body_lines.join("\n");
    // Preserve trailing newline shape of the original body.
    let pruned = if body.ends_with('\n') && !pruned.ends_with('\n') {
        format!("{pruned}\n")
    } else {
        pruned
    };
    let healed = heal_managed_embed(&pruned, route, block_id);
    let _ = managed_embed_line(route, block_id);

    // Frontmatter: set parent, recompute hash/base when present.
    let mut front_lines: Vec<String> = front_opt.unwrap_or_default();
    let parent_line = format!("parent: \"[[{route}]]\"");
    let mut saw_parent = false;
    for line in front_lines.iter_mut() {
        if line
            .split_once(':')
            .is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case("parent"))
        {
            *line = parent_line.clone();
            saw_parent = true;
        }
    }
    if !saw_parent {
        front_lines.push(parent_line);
    }
    // Recompute parent-free hash/base when both lines exist.
    let has_hash = front_lines.iter().any(|l| {
        l.split_once(':')
            .is_some_and(|(k, _)| k.trim() == "highlights_marker_hash")
    });
    let has_base = front_lines.iter().any(|l| {
        l.split_once(':')
            .is_some_and(|(k, _)| k.trim() == "highlights_marker_base")
    });
    if has_hash && has_base {
        if let Some(updated) = recompute_parent_free_hash(&front_lines) {
            front_lines = updated;
        }
    }
    let front_joined = front_lines.join("\n");
    Ok(format!("---\n{front_joined}\n---\n{healed}"))
}

/// Recompute `highlights_marker_hash` from the parent-free base snapshot.
fn recompute_parent_free_hash(front_lines: &[String]) -> Option<Vec<String>> {
    let mut base_json: Option<String> = None;
    for line in front_lines {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim() == "highlights_marker_base" {
                let mut v = v.trim().to_string();
                if (v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2)
                    || (v.starts_with('"') && v.ends_with('"') && v.len() >= 2)
                {
                    v = v[1..v.len() - 1].to_string();
                }
                // Unescape single-quoted YAML escaping.
                v = v.replace("''", "'");
                base_json = Some(v);
            }
        }
    }
    let base_json = base_json?;
    let value: serde_json::Value = serde_json::from_str(&base_json).ok()?;
    let serde_json::Value::Object(mut map) = value else {
        return None;
    };
    map.remove("parent");
    let canonical =
        serde_json::to_string(&serde_json::Value::Object(map)).ok()?;
    let hash = hex::encode(sha2::Sha256::digest(canonical.as_bytes()));
    let mut out: Vec<String> = Vec::new();
    for line in front_lines {
        if let Some((k, _)) = line.split_once(':') {
            if k.trim() == "highlights_marker_hash" {
                // Preserve quoting style: single-quoted string.
                out.push(format!("highlights_marker_hash: '{hash}'"));
                continue;
            }
        }
        out.push(line.clone());
    }
    Some(out)
}

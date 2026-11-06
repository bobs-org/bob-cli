//! Pure planning for resolving a successor offer as Done or Cancel.
//!
//! Offer verdicts bind to the source row shown by the picker. This module
//! owns the shared eligibility, consequence copy, root-only line edits, and
//! recover-only cascade used by capture and the Obsidian cycler.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde::Serialize;

use crate::native::{
    capture::line_spans,
    capture_task_toggle::{child_block_end_line, set_task_line_status},
    note_tasks::{self, NoteTaskSettings},
    project_dependencies::is_project_task_id,
    task_cancel,
    task_status_hooks::{
        self, fenced_lines, logical_lines, TaskLine, TasksSettings,
    },
    vault_links::NoteIndex,
};

use super::{
    successors::CompletedTask, SuccessorInput, SuccessorPlan,
    SuccessorSelection,
};

const NO_REASON_GIVEN: &str = "🤷 no reason given";

/// Bob-authored close eligibility and consequence copy for one offer row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct OfferCloseDescriptor {
    pub(crate) complete: bool,
    pub(crate) cancel: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) why: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) complete_consequence: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cancel_consequence: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) scope_note: Option<String>,
    pub(crate) unblocks: usize,
}

impl OfferCloseDescriptor {
    pub(crate) fn refused(why: impl Into<String>) -> Self {
        Self {
            complete: false,
            cancel: false,
            why: Some(why.into()),
            complete_consequence: None,
            cancel_consequence: None,
            scope_note: None,
            unblocks: 0,
        }
    }
}

/// One row and the immutable planning context used to describe or resolve it.
pub(crate) struct OfferCloseInput<'a> {
    /// Stable `path#^id` or `path@digest~n` offer identity.
    pub(crate) key: &'a str,
    pub(crate) path: &'a Path,
    /// Zero-based line index of the row that was shown to the user.
    pub(crate) line_index: usize,
    /// Exact task line shown to the user, without its line ending.
    pub(crate) expected_line: &'a str,
    /// The staged source note at offer time.
    pub(crate) source_text: &'a str,
    pub(crate) group: &'a str,
    pub(crate) is_new_task: bool,
    pub(crate) muted: bool,
    pub(crate) today: NaiveDate,
    pub(crate) tasks_settings: &'a TasksSettings,
    pub(crate) note_settings: &'a NoteTaskSettings,
    /// Vault-relative note snapshots, including `path`.
    pub(crate) snapshot: &'a [(PathBuf, String)],
    pub(crate) snapshot_complete: bool,
    pub(crate) day_relative: &'a Path,
    pub(crate) index: &'a NoteIndex,
}

/// The staged card verdict to apply in the capture write pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OfferCloseAction {
    Done,
    Cancel { reason: String },
}

/// Why a requested card close could not be planned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OfferCloseRefusal {
    ChangedIdentity,
    Muted,
    NewTask,
    Recurring,
    Status,
}

impl OfferCloseRefusal {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::ChangedIdentity => "changed_identity",
            Self::Muted => "muted",
            Self::NewTask => "new_task",
            Self::Recurring => "recurring",
            Self::Status => "status",
        }
    }
}

/// Root-only result plus the ordinary recover-only cascade. The caller
/// composes `source_postimage` with `recovery.changed_files` in its existing
/// atomic write pass; no ledger or Task Link mutation is included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OfferClosePlan {
    pub(crate) descriptor: OfferCloseDescriptor,
    pub(crate) action: OfferCloseAction,
    pub(crate) source_postimage: String,
    pub(crate) recovery: SuccessorPlan,
    pub(crate) previous_status: char,
    pub(crate) block_id: String,
    pub(crate) note_path: PathBuf,
    pub(crate) reason: Option<String>,
}

/// Describe Done and Cancel independently for one selectable offer.
pub(crate) fn describe_offer_close(
    input: &OfferCloseInput<'_>,
) -> OfferCloseDescriptor {
    if input.muted {
        return OfferCloseDescriptor::refused(
            OfferCloseRefusal::Muted.as_str(),
        );
    }
    if input.is_new_task {
        return OfferCloseDescriptor::refused(
            OfferCloseRefusal::NewTask.as_str(),
        );
    }
    let task = match resolve_offer_task(input) {
        Ok(task) => task,
        Err(refusal) => return OfferCloseDescriptor::refused(refusal.as_str()),
    };
    let raw_line =
        logical_line_at(input.source_text, task.line_index).unwrap_or_default();
    if super::is_recurring_task_line(raw_line) {
        return OfferCloseDescriptor::refused(
            OfferCloseRefusal::Recurring.as_str(),
        );
    }
    let complete = super::verdicts::complete_verdict(
        raw_line,
        input.tasks_settings,
        false,
    );
    let cancel = task_cancel::verdicts::cancel_verdict(
        raw_line,
        input.tasks_settings,
        false,
    );
    let eligible = matches!(
        complete,
        super::verdicts::CompleteSourceVerdict::Completes { .. }
    ) && matches!(
        cancel,
        task_cancel::verdicts::CancelSourceVerdict::Cancels { .. }
    );
    if !eligible {
        return OfferCloseDescriptor::refused(
            OfferCloseRefusal::Status.as_str(),
        );
    }

    let status_name = status_name(input.note_settings, task.status);
    let task_text = note_tasks::clean_description(
        &task.description,
        &input.note_settings.global_filter,
        task.block_id.as_deref(),
    );
    let complete_consequence = if input.group == "project_finished"
        && is_project_task_id(task.block_id.as_deref())
    {
        format!("✓ Finishes ‹{}›", project_name(input.path))
    } else {
        format!("✓ Completes “{task_text}” · {status_name} → Done")
    };
    let cancel_consequence =
        format!("⊘ Cancels “{task_text}” · {status_name} → Cancelled");
    let scope_note = open_scope_count(
        input.source_text,
        task.line_index,
        task.block_id.as_deref(),
        input.note_settings,
    )
    .filter(|count| *count > 0)
    .map(|count| {
        let (noun, verb) = if count == 1 {
            ("task", "stays")
        } else {
            ("tasks", "stay")
        };
        format!("· its {count} embedded {noun} {verb} open")
    });
    let unblocks = if input.snapshot_complete {
        recover_for_single_close(input, &task, raw_line, &complete_consequence)
            .map(|plan| plan.unblocked.len())
            .unwrap_or(0)
    } else {
        0
    };
    OfferCloseDescriptor {
        complete: true,
        cancel: true,
        why: None,
        complete_consequence: Some(complete_consequence),
        cancel_consequence: Some(cancel_consequence),
        scope_note,
        unblocks,
    }
}

/// Apply a staged verdict against the exact task row shown in the card.
pub(crate) fn plan_offer_close(
    input: &OfferCloseInput<'_>,
    action: OfferCloseAction,
) -> Result<OfferClosePlan, OfferCloseRefusal> {
    let task = resolve_offer_task(input)?;
    let descriptor = describe_offer_close(input);
    if !descriptor.complete {
        return Err(match descriptor.why.as_deref() {
            Some("muted") => OfferCloseRefusal::Muted,
            Some("new_task") => OfferCloseRefusal::NewTask,
            Some("recurring") => OfferCloseRefusal::Recurring,
            Some("status") => OfferCloseRefusal::Status,
            _ => OfferCloseRefusal::ChangedIdentity,
        });
    }
    let raw_line = logical_line_at(input.source_text, task.line_index)
        .ok_or(OfferCloseRefusal::ChangedIdentity)?;
    let source_postimage = match &action {
        OfferCloseAction::Done => edit_done_root(
            input.source_text,
            task.line_index,
            input.today,
            raw_line,
            input.note_settings,
        )
        .ok_or(OfferCloseRefusal::ChangedIdentity)?,
        OfferCloseAction::Cancel { reason } => edit_cancel_root(
            input.source_text,
            task.line_index,
            input.today,
            reason,
        ),
    };
    let completed = CompletedTask {
        relative_path: input.path.to_path_buf(),
        block_id: task.block_id.clone().unwrap_or_default(),
        task_id: task.task_id.clone().unwrap_or_default(),
        text: task.description.clone(),
        is_root: true,
    };
    let mut snapshot = input.snapshot.to_vec();
    if let Some((_, contents)) =
        snapshot.iter_mut().find(|(path, _)| path == input.path)
    {
        *contents = source_postimage.clone();
    } else {
        snapshot.push((input.path.to_path_buf(), source_postimage.clone()));
    }
    let mut cancelled_projects = BTreeSet::new();
    if matches!(action, OfferCloseAction::Cancel { .. })
        && is_project_task_id(task.block_id.as_deref())
    {
        cancelled_projects.insert(input.path.to_path_buf());
    }
    let recovery = super::successors::plan_recovery_only(
        SuccessorInput {
            bob_dir: None,
            completed: vec![completed],
            snapshot,
            note_texts: Vec::new(),
            pre_day: None,
            post_day: None,
            closing: None,
            day_relative: input.day_relative.to_path_buf(),
            today: input.today,
            tasks_settings: input.tasks_settings,
            note_settings: input.note_settings,
            link_unblocked: false,
            keep_moving: false,
            snapshot_complete: true,
            selection: SuccessorSelection::None,
            index: input.index,
            relation: None,
        },
        &cancelled_projects,
    );
    let reason = match &action {
        OfferCloseAction::Done => None,
        OfferCloseAction::Cancel { reason } => {
            let reason = one_line_reason(reason);
            (!reason.is_empty()).then_some(reason)
        }
    };
    Ok(OfferClosePlan {
        descriptor,
        action,
        source_postimage,
        recovery,
        previous_status: task.status,
        block_id: task.block_id.unwrap_or_default(),
        note_path: input.path.to_path_buf(),
        reason,
    })
}

fn resolve_offer_task(
    input: &OfferCloseInput<'_>,
) -> Result<TaskLine, OfferCloseRefusal> {
    let Some((relative, contents)) =
        input.snapshot.iter().find(|(path, _)| path == input.path)
    else {
        return Err(OfferCloseRefusal::ChangedIdentity);
    };
    if contents != input.source_text {
        return Err(OfferCloseRefusal::ChangedIdentity);
    }
    let Some(raw_line) = logical_line_at(contents, input.line_index) else {
        return Err(OfferCloseRefusal::ChangedIdentity);
    };
    if raw_line != input.expected_line {
        return Err(OfferCloseRefusal::ChangedIdentity);
    }
    let tasks = task_status_hooks::parse_tasks(contents, input.tasks_settings);
    let Some(task) = tasks
        .iter()
        .find(|task| task.line_index == input.line_index)
    else {
        return Err(OfferCloseRefusal::ChangedIdentity);
    };
    if !offer_key_matches(input.key, relative, task, input.note_settings) {
        return Err(OfferCloseRefusal::ChangedIdentity);
    }
    Ok(task.clone())
}

fn offer_key_matches(
    key: &str,
    relative: &Path,
    task: &TaskLine,
    settings: &NoteTaskSettings,
) -> bool {
    if let Some(block_id) = task.block_id.as_deref()
        && key
            .rsplit_once("#^")
            .is_some_and(|(_, key_id)| key_id == block_id)
    {
        return true;
    }
    let Some((_key_path, identity)) = key.rsplit_once('@') else {
        return false;
    };
    let (digest, ordinal) = identity
        .split_once('~')
        .map(|(digest, ordinal)| (digest, ordinal.parse::<usize>().ok()))
        .unwrap_or((identity, Some(1)));
    if !ordinal.is_some_and(|ordinal| ordinal > 0)
        || digest.len() != 8
        || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return false;
    }
    let canonical = note_tasks::stable_task_key(
        relative,
        None,
        &task.description,
        &settings.global_filter,
        1,
    );
    canonical
        .rsplit_once('@')
        .is_some_and(|(_, expected)| expected == digest)
}

fn recover_for_single_close(
    input: &OfferCloseInput<'_>,
    task: &TaskLine,
    raw_line: &str,
    _consequence: &str,
) -> Option<SuccessorPlan> {
    if task.task_id.is_none() && !is_project_task_id(task.block_id.as_deref()) {
        return Some(SuccessorPlan::default());
    }
    let source_postimage = edit_done_root(
        input.source_text,
        task.line_index,
        input.today,
        raw_line,
        input.note_settings,
    )?;
    let completed = CompletedTask {
        relative_path: input.path.to_path_buf(),
        block_id: task.block_id.clone().unwrap_or_default(),
        task_id: task.task_id.clone().unwrap_or_default(),
        text: task.description.clone(),
        is_root: true,
    };
    let mut snapshot = input.snapshot.to_vec();
    if let Some((_, contents)) =
        snapshot.iter_mut().find(|(path, _)| path == input.path)
    {
        *contents = source_postimage;
    }
    let recovery = super::successors::plan_recovery_only(
        SuccessorInput {
            bob_dir: None,
            completed: vec![completed],
            snapshot,
            note_texts: Vec::new(),
            pre_day: None,
            post_day: None,
            closing: None,
            day_relative: input.day_relative.to_path_buf(),
            today: input.today,
            tasks_settings: input.tasks_settings,
            note_settings: input.note_settings,
            link_unblocked: false,
            keep_moving: false,
            snapshot_complete: true,
            selection: SuccessorSelection::None,
            index: input.index,
            relation: None,
        },
        &BTreeSet::new(),
    );
    Some(recovery)
}

fn edit_done_root(
    source_text: &str,
    line_index: usize,
    today: NaiveDate,
    line: &str,
    note_settings: &NoteTaskSettings,
) -> Option<String> {
    let mut updated = set_task_line_status(line, 'x')?;
    if super::tree::task_has_global_filter(line, note_settings) {
        updated = super::tree::add_or_replace_completion_field(
            &updated,
            &today.format("%Y-%m-%d").to_string(),
        );
    }
    replace_line(source_text, line_index, &updated)
}

fn edit_cancel_root(
    source_text: &str,
    line_index: usize,
    today: NaiveDate,
    reason: &str,
) -> String {
    let reason = one_line_reason(reason);
    let reasons =
        if reason.is_empty() && has_cancel_log(source_text, line_index) {
            vec![NO_REASON_GIVEN.to_string()]
        } else if reason.is_empty() {
            Vec::new()
        } else {
            vec![reason]
        };
    let edited = if reasons.is_empty() {
        task_cancel::line::edit_source(source_text, line_index, today, &reasons)
    } else {
        task_cancel::line::edit_source_create_log(
            source_text,
            line_index,
            today,
            &reasons,
        )
    };
    edited.content
}

fn has_cancel_log(source_text: &str, line_index: usize) -> bool {
    let lines = logical_lines(source_text);
    if line_index >= lines.len() {
        return false;
    }
    let fenced = fenced_lines(&lines, 0..lines.len());
    crate::native::task_dependencies::blocks::direct_child_lines(
        &lines, &fenced, line_index,
    )
    .into_iter()
    .any(|index| {
        crate::native::task_dependencies::blocks::is_cancel_log(lines[index])
    })
}

fn replace_line(
    content: &str,
    index: usize,
    replacement: &str,
) -> Option<String> {
    let mut segments = content
        .split_inclusive('\n')
        .map(str::to_string)
        .collect::<Vec<_>>();
    let segment = segments.get_mut(index)?;
    let ending = if segment.ends_with("\r\n") {
        "\r\n"
    } else if segment.ends_with('\n') {
        "\n"
    } else {
        ""
    };
    *segment = format!("{replacement}{ending}");
    Some(segments.concat())
}

fn logical_line_at(contents: &str, index: usize) -> Option<&str> {
    let segment = contents.split_inclusive('\n').nth(index)?;
    let line = segment.strip_suffix('\n').unwrap_or(segment);
    Some(line.strip_suffix('\r').unwrap_or(line))
}

fn one_line_reason(reason: &str) -> String {
    reason.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn status_name(settings: &NoteTaskSettings, symbol: char) -> String {
    settings
        .status_definitions
        .iter()
        .find(|definition| definition.symbol == symbol.to_string())
        .map(|definition| definition.name.clone())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| match symbol {
            ' ' => "Ready".to_string(),
            '?' => "Blocked".to_string(),
            '*' => "Next".to_string(),
            '/' => "In Progress".to_string(),
            'x' | 'X' => "Done".to_string(),
            '-' => "Cancelled".to_string(),
            _ => "Unknown".to_string(),
        })
}

fn project_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("project")
        .to_string()
}

fn open_scope_count(
    source_text: &str,
    line_index: usize,
    block_id: Option<&str>,
    settings: &NoteTaskSettings,
) -> Option<usize> {
    let _task = note_tasks::scan(source_text, settings)
        .tasks()
        .iter()
        .cloned()
        .into_iter()
        .find(|task| task.line_index == line_index)?;
    let tasks = task_status_hooks::parse_tasks(source_text, settings);
    if is_project_task_id(block_id) {
        return Some(
            tasks
                .iter()
                .filter(|candidate| {
                    candidate.line_index != line_index
                        && candidate.status_type.is_open()
                })
                .count(),
        );
    }
    let spans = line_spans(source_text);
    let end = child_block_end_line(&spans, line_index);
    let parent_indent = leading_whitespace_len(spans.get(line_index)?.text);
    Some(
        tasks
            .iter()
            .filter(|candidate| {
                candidate.line_index > line_index
                    && candidate.line_index <= end
                    && leading_whitespace_len(spans[candidate.line_index].text)
                        > parent_indent
                    && candidate.status_type.is_open()
            })
            .count(),
    )
}

fn leading_whitespace_len(line: &str) -> usize {
    line.bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count()
}

//! Keep-moving successor offers (`docs/task-dependencies.md` §12.15–§12.17).
//!
//! Leaf detection, the G1–G3 no-I/O pre-gate, and candidate classification
//! live here. [`super::successors`] still owns routing, keys, selection,
//! placement, and writes. Unpicked keep-moving rows never recover or mint.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use super::super::capture_dependency_tasks::{
    has_hide_tag, has_prj_is_blocked_tag,
};
use super::super::pomodoro_homes::{
    path_string, DiskHomeSource, HomeKind, HomeResolver, HomeSource,
};
use super::super::project_dependencies::{
    blocking_open_dependency_ids, is_project_task_id, ProjectBlockReasons,
};
use super::super::projects::{
    frontmatter_is_project, frontmatter_value, parse_frontmatter,
    project_surface, ProjectStatus,
};
use super::super::task_status_hooks::{
    parse_tasks, FileScan, TaskDependencyState, TaskLine, TasksSettings,
};
use super::super::vault_links::{LinkResolution, NoteIndex};
use super::project_close::ProjectCloseStates;
use super::successors::{
    is_inbox_note, raw_task_line, CompletedTask, DependentVerdict, OfferGroup,
};

/// Trigger every keep-moving row reports (`docs/task-dependencies.md` §12.15).
pub(crate) const KEEP_MOVING_TRIGGER: &str = "last_planned";

/// Folded counts for one keep-moving project with at least one selectable row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeepMovingFold {
    pub project: PathBuf,
    pub hidden: usize,
    pub blocked: usize,
    pub scheduled: usize,
}

/// One keep-moving offer before destination / key assignment.
#[derive(Debug, Clone)]
pub(crate) struct KeepMovingOffer {
    pub file: usize,
    pub task: usize,
    pub preds: Vec<usize>,
    pub project: PathBuf,
}

/// Classification of keep-moving offers after existing groups exist.
#[derive(Debug, Clone, Default)]
pub(crate) struct KeepMovingClassification {
    pub offers: Vec<KeepMovingOffer>,
    pub folds: Vec<KeepMovingFold>,
}

/// Inputs the keep-moving classifier reads from the successor planner.
pub(crate) struct KeepMovingClassInput<'a> {
    pub completed: &'a [CompletedTask],
    pub files: &'a [FileScan],
    pub surfaces: &'a [Option<super::super::projects::ProjectSurface>],
    pub project_states: &'a ProjectCloseStates,
    pub states: &'a BTreeMap<(usize, usize), TaskDependencyState>,
    pub verdicts: &'a [DependentVerdict],
    pub live: &'a BTreeMap<(PathBuf, String), (usize, usize, usize, String)>,
    pub today: NaiveDate,
    pub snapshot_complete: bool,
    pub link_unblocked: bool,
    pub index: &'a NoteIndex,
}

/// Staged-first home source: touched notes from the planner snapshot, disk
/// only for ancestors the walk actually reaches.
pub(crate) struct KeepMovingHomeSource<'a> {
    staged: BTreeMap<PathBuf, &'a str>,
    index: &'a NoteIndex,
    disk: Option<DiskHomeSource>,
}

impl<'a> KeepMovingHomeSource<'a> {
    pub(crate) fn new(
        files: &'a [FileScan],
        index: &'a NoteIndex,
        bob_dir: Option<&Path>,
    ) -> Self {
        let staged = files
            .iter()
            .map(|file| (file.relative_path.clone(), file.contents.as_str()))
            .collect();
        Self {
            staged,
            index,
            disk: bob_dir.map(DiskHomeSource::new),
        }
    }
}

impl HomeSource for KeepMovingHomeSource<'_> {
    fn resolve(&self, current: &Path, target: &str) -> LinkResolution {
        let from_index = self.index.resolve_detailed(Some(current), target);
        if !matches!(from_index, LinkResolution::Missing) {
            return from_index;
        }
        match &self.disk {
            Some(disk) => disk.resolve(current, target),
            None => from_index,
        }
    }

    fn read_facts(
        &self,
        path: &Path,
    ) -> Option<super::super::pomodoro_homes::NoteFacts> {
        if let Some(contents) = self.staged.get(path) {
            return Some(super::super::pomodoro_homes::NoteFacts::parse(
                contents,
            ));
        }
        self.disk.as_ref()?.read_facts(path)
    }
}

/// Test seam: counts every `read_facts` call, including the starting note.
#[cfg(test)]
pub(crate) struct CountingHomeSource<'a> {
    inner: super::super::pomodoro_homes::IndexedHomeSource<'a>,
    pub reads: std::cell::Cell<u32>,
}

#[cfg(test)]
impl<'a> CountingHomeSource<'a> {
    pub(crate) fn new(
        note_index: &'a NoteIndex,
        contents: &'a BTreeMap<PathBuf, String>,
    ) -> Self {
        Self {
            inner: super::super::pomodoro_homes::IndexedHomeSource::new(
                note_index, contents,
            ),
            reads: std::cell::Cell::new(0),
        }
    }
}

#[cfg(test)]
impl HomeSource for CountingHomeSource<'_> {
    fn resolve(&self, current: &Path, target: &str) -> LinkResolution {
        self.inner.resolve(current, target)
    }

    fn read_facts(
        &self,
        path: &Path,
    ) -> Option<super::super::pomodoro_homes::NoteFacts> {
        self.reads.set(self.reads.get() + 1);
        self.inner.read_facts(path)
    }
}

/// Lane pull order for keep-moving rows: Pending → Next → Ready.
pub(crate) fn keep_moving_lane(status: char) -> u8 {
    match status {
        '/' => 0,
        '*' => 1,
        ' ' => 2,
        _ => 3,
    }
}

/// G1–G3 no-I/O pre-gate over held completed-note and baseline day text.
/// Leafness and live-link parsing happen only after this returns true.
pub(crate) fn keep_moving_pre_gate(
    completed: &[CompletedTask],
    note_texts: &[(PathBuf, String)],
    pre_day: Option<&str>,
    today: NaiveDate,
    tasks_settings: &TasksSettings,
) -> bool {
    let Some(day_text) = pre_day else {
        return false;
    };
    completed.iter().any(|task| {
        g1_predecessor(task, note_texts)
            && day_text.contains(&format!("#^{}", task.block_id))
            && g2_has_candidate(
                task,
                completed,
                note_texts,
                today,
                tasks_settings,
            )
    })
}

/// Whether proving a triggered project safe needs the vault-wide dependents
/// snapshot (`docs/task-dependencies.md` §12.15 "Prerequisite checks").
/// Only an open task of a completed task's note that carries a non-empty
/// `[dependsOn::]` (which the managed Depends-On line mirrors onto `^prj`)
/// has prerequisites to prove closed; project inheritance reads the
/// `#prj_is_blocked` tag from held text. Held text only, no I/O, so the
/// common triggering close pays for no snapshot.
pub(crate) fn keep_moving_needs_dependents(
    completed: &[CompletedTask],
    note_texts: &[(PathBuf, String)],
    tasks_settings: &TasksSettings,
) -> bool {
    note_texts.iter().any(|(path, text)| {
        completed.iter().any(|task| &task.relative_path == path)
            && parse_tasks(text, tasks_settings).iter().any(|task| {
                task.status_recognized
                    && task.status_type.is_open()
                    && task
                        .depends_on
                        .iter()
                        .any(|dependency| !dependency.is_empty())
            })
    })
}

/// Whether N is a leaf project: recognised, non-terminal, outside `done/`,
/// one open `^prj`, Project kind, own path as key, different root parent.
pub(crate) fn is_leaf_project<S: HomeSource>(
    resolver: &mut HomeResolver<S>,
    path: &Path,
    contents: &str,
    today: NaiveDate,
    index: &NoteIndex,
) -> bool {
    if in_done_archive(path) {
        return false;
    }
    if is_inbox_note(path, contents, index) {
        return false;
    }
    if project_surface(path, contents, today).is_none() {
        return false;
    }
    let home = resolver.resolve_note(path);
    home.kind == HomeKind::Project
        && home.key == path_string(path)
        && home.root_parent != home.key
}

/// Classify keep-moving offers after hand-off, project, and pull-in groups
/// exist so K7 and dedup can see them.
pub(crate) fn classify_keep_moving<S: HomeSource>(
    resolver: &mut HomeResolver<S>,
    input: KeepMovingClassInput<'_>,
) -> KeepMovingClassification {
    if !input.link_unblocked {
        return KeepMovingClassification::default();
    }
    let mut classified = KeepMovingClassification::default();
    let mut seen_notes = BTreeMap::new();
    for (position, completed) in input.completed.iter().enumerate() {
        seen_notes
            .entry(completed.relative_path.clone())
            .or_insert_with(Vec::new)
            .push(position);
    }
    for (path, pred_positions) in seen_notes {
        classify_one_note(
            resolver,
            &input,
            &path,
            &pred_positions,
            &mut classified,
        );
    }
    classified
}

fn classify_one_note<S: HomeSource>(
    resolver: &mut HomeResolver<S>,
    input: &KeepMovingClassInput<'_>,
    path: &Path,
    pred_positions: &[usize],
    classified: &mut KeepMovingClassification,
) {
    let Some(file_index) = input
        .files
        .iter()
        .position(|file| file.relative_path == path)
    else {
        return;
    };
    let file = &input.files[file_index];
    let Some(surface) = input.surfaces.get(file_index).and_then(|s| s.as_ref())
    else {
        return;
    };
    if surface.is_empty() {
        return;
    }
    if project_already_grouped(input.verdicts, file_index, path) {
        return;
    }
    if !is_leaf_project(
        resolver,
        path,
        &file.contents,
        input.today,
        input.index,
    ) {
        return;
    }
    let Some(prj_index) = file
        .tasks
        .iter()
        .position(|task| is_project_task_id(task.block_id.as_deref()))
    else {
        return;
    };
    let prj = &file.tasks[prj_index];
    if pred_in_c(input.completed, path, prj) {
        return;
    }
    if project_root_unsafe(input, file_index, file, prj) {
        return;
    }
    let preds: Vec<usize> = pred_positions
        .iter()
        .copied()
        .filter(|&position| {
            let completed = &input.completed[position];
            predecessor_qualifies(completed, prj, input.live)
        })
        .collect();
    if preds.is_empty() {
        return;
    }
    if k4_other_open_work(input, file, path) {
        return;
    }
    let mut hidden = 0usize;
    let mut blocked = 0usize;
    let mut scheduled = 0usize;
    let mut offers = Vec::new();
    let context = &input.project_states[file_index].context;
    for (task_index, task) in file.tasks.iter().enumerate() {
        if !context.is_resident(task) {
            continue;
        }
        if pred_in_c(input.completed, path, task) {
            continue;
        }
        if already_classified(input.verdicts, file_index, task_index) {
            continue;
        }
        match classify_resident(input, file_index, task_index, file, task) {
            ResidentClass::Ignored => {}
            ResidentClass::Hidden => hidden += 1,
            ResidentClass::Blocked => blocked += 1,
            ResidentClass::Scheduled => scheduled += 1,
            ResidentClass::Offer => offers.push(KeepMovingOffer {
                file: file_index,
                task: task_index,
                preds: preds.clone(),
                project: path.to_path_buf(),
            }),
        }
    }
    if offers.is_empty() {
        return;
    }
    classified.offers.extend(offers);
    classified.folds.push(KeepMovingFold {
        project: path.to_path_buf(),
        hidden,
        blocked,
        scheduled,
    });
}

enum ResidentClass {
    Ignored,
    Hidden,
    Blocked,
    Scheduled,
    Offer,
}

fn classify_resident(
    input: &KeepMovingClassInput<'_>,
    file_index: usize,
    task_index: usize,
    file: &FileScan,
    task: &TaskLine,
) -> ResidentClass {
    if !matches!(task.status, ' ' | '*' | '/' | '?') {
        return ResidentClass::Ignored;
    }
    let raw = raw_task_line(&file.contents, task.line_index);
    if task.status == '?' || has_prj_is_blocked_tag(&raw) {
        return ResidentClass::Blocked;
    }
    if !task.depends_on.is_empty() && !input.snapshot_complete {
        return ResidentClass::Blocked;
    }
    let open_count = blocking_open_dependency_ids(
        file,
        task,
        input
            .states
            .get(&(file_index, task_index))
            .map(|state| state.open_dependency_ids.as_slice())
            .unwrap_or(&[]),
    )
    .len();
    if open_count > 0 {
        return ResidentClass::Blocked;
    }
    if task.scheduled.is_some_and(|date| date > input.today) {
        return ResidentClass::Scheduled;
    }
    if has_hide_tag(&raw) {
        return ResidentClass::Hidden;
    }
    ResidentClass::Offer
}

fn project_root_unsafe(
    input: &KeepMovingClassInput<'_>,
    file_index: usize,
    file: &FileScan,
    prj: &TaskLine,
) -> bool {
    let raw = raw_task_line(&file.contents, prj.line_index);
    if has_prj_is_blocked_tag(&raw) {
        return true;
    }
    let Some(state) = input.project_states.get(file_index) else {
        return true;
    };
    if state.inherited.is_some() {
        return true;
    }
    if !prj.depends_on.is_empty() && !input.snapshot_complete {
        return true;
    }
    let reasons: &ProjectBlockReasons = &state.reasons;
    !reasons.is_empty()
}

fn predecessor_qualifies(
    completed: &CompletedTask,
    prj: &TaskLine,
    live: &BTreeMap<(PathBuf, String), (usize, usize, usize, String)>,
) -> bool {
    if completed.block_id.is_empty() {
        return false;
    }
    if is_project_task_id(Some(completed.block_id.as_str())) {
        return false;
    }
    if prj.block_id.as_deref() == Some(completed.block_id.as_str()) {
        return false;
    }
    live.contains_key(&(
        completed.relative_path.clone(),
        completed.block_id.clone(),
    ))
}

fn k4_other_open_work(
    input: &KeepMovingClassInput<'_>,
    file: &FileScan,
    path: &Path,
) -> bool {
    let c_ids: Vec<&str> = input
        .completed
        .iter()
        .filter(|task| task.relative_path == path)
        .map(|task| task.block_id.as_str())
        .collect();
    input.live.keys().any(|(note, block_id)| {
        if note != path {
            return false;
        }
        if c_ids.contains(&block_id.as_str()) {
            return false;
        }
        let Some(task) = file
            .tasks
            .iter()
            .find(|task| task.block_id.as_deref() == Some(block_id.as_str()))
        else {
            return false;
        };
        task.status_recognized && task.status_type.is_open()
    })
}

fn pred_in_c(
    completed: &[CompletedTask],
    path: &Path,
    task: &TaskLine,
) -> bool {
    completed.iter().any(|c| {
        c.relative_path == path
            && (task.block_id.as_deref().is_some_and(|id| id == c.block_id)
                || task
                    .task_id
                    .as_deref()
                    .is_some_and(|id| !id.is_empty() && id == c.task_id))
    })
}

fn already_classified(
    verdicts: &[DependentVerdict],
    file: usize,
    task: usize,
) -> bool {
    verdicts
        .iter()
        .any(|verdict| verdict.file == file && verdict.task == task)
}

fn project_already_grouped(
    verdicts: &[DependentVerdict],
    file_index: usize,
    path: &Path,
) -> bool {
    verdicts.iter().any(|verdict| {
        if verdict.pull_in && verdict.file == file_index {
            return true;
        }
        verdict.offer.as_ref().is_some_and(|offer| {
            matches!(
                offer.group,
                OfferGroup::Project | OfferGroup::ProjectFinished
            ) && (offer.project.as_deref() == Some(path)
                || (offer.project.is_none() && verdict.file == file_index))
        })
    })
}

fn g1_predecessor(
    task: &CompletedTask,
    note_texts: &[(PathBuf, String)],
) -> bool {
    if task.block_id.is_empty()
        || is_project_task_id(Some(task.block_id.as_str()))
    {
        return false;
    }
    if in_done_archive(&task.relative_path) {
        return false;
    }
    let Some(contents) = note_text(note_texts, &task.relative_path) else {
        return false;
    };
    g1_project_frontmatter(contents)
}

fn g1_project_frontmatter(contents: &str) -> bool {
    let Some(frontmatter) = parse_frontmatter(contents) else {
        return false;
    };
    frontmatter_is_project(&frontmatter)
        && !ProjectStatus::parse(frontmatter_value(&frontmatter, "status"))
            .is_terminal()
}

fn g2_has_candidate(
    completed: &CompletedTask,
    all: &[CompletedTask],
    note_texts: &[(PathBuf, String)],
    today: NaiveDate,
    tasks_settings: &TasksSettings,
) -> bool {
    let Some(contents) = note_text(note_texts, &completed.relative_path) else {
        return false;
    };
    let tasks = parse_tasks(contents, tasks_settings);
    let c_ids: Vec<&str> = all
        .iter()
        .filter(|task| task.relative_path == completed.relative_path)
        .map(|task| task.block_id.as_str())
        .collect();
    tasks.iter().any(|task| {
        if is_project_task_id(task.block_id.as_deref()) {
            return false;
        }
        if task
            .block_id
            .as_deref()
            .is_some_and(|id| c_ids.contains(&id))
        {
            return false;
        }
        if !matches!(task.status, ' ' | '*' | '/') {
            return false;
        }
        let raw = contents.lines().nth(task.line_index).unwrap_or_default();
        if has_hide_tag(raw) {
            return false;
        }
        !task.scheduled.is_some_and(|date| date > today)
    })
}

fn note_text<'a>(
    notes: &'a [(PathBuf, String)],
    path: &Path,
) -> Option<&'a str> {
    notes
        .iter()
        .find(|(note, _)| note == path)
        .map(|(_, contents)| contents.as_str())
}

fn in_done_archive(relative: &Path) -> bool {
    relative
        .components()
        .any(|component| component.as_os_str() == "done")
}

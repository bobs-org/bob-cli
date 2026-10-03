//! R1–R10 Depends-On reconciliation (`docs/task-dependencies.md` §4).
//!
//! Before edges and Blocked derivation, every open dependent's
//! Depends-On line is projected into its `[dependsOn::]` / `[id::]`
//! fields: adopt (R2), heal (R3), canonicalise, warn (R4–R10), and
//! report. Closed dependents are never touched. The previous daily
//! note is never written. Edits apply to note contents up front so the
//! existing parse → edges → Blocked pipeline reads reconciled text;
//! touched notes flow through the normal guarded write and count as
//! structural, so the quiet interval defers recently modified notes.
use super::*;

use std::collections::BTreeMap;

mod apply;
mod fields;

pub(crate) use fields::{
    is_cancel_log_line, rebuild_child_line, set_task_fields,
};

/// Outcome of one reconciliation pass: content edits already applied
/// to `files`, plus the report rows the sync result prints.
pub(super) struct ReconcileOutcome {
    pub(super) touched_files: BTreeSet<usize>,
    pub(super) projection_updates: Vec<DependencyProjectionUpdate>,
    pub(super) adopted: Vec<DependencyLineReport>,
    pub(super) healed: Vec<DependencyLineReport>,
    pub(super) canonicalized: Vec<DependencyLineReport>,
    pub(super) legacy_children: usize,
    pub(super) warnings: Vec<DependencyWarning>,
    /// Vault-wide ids of archive (`done/`) prerequisites: kept in the
    /// field, never warned about, never blocking.
    pub(super) archive_closed_ids: BTreeSet<String>,
    /// Original note contents for touched files only, so callers clone
    /// just what reconcile rewrote instead of the whole vault.
    pub(super) original_contents: BTreeMap<usize, String>,
}

/// Project every open dependent's Depends-On line into its fields.
///
/// `files` is mutated in place (note contents only; paths never
/// change), so callers must re-parse tasks and rebuild derived maps
/// afterwards.
pub(super) fn reconcile_dependencies(
    vault: &Path,
    files: &mut [FileScan],
    note_index: &NoteIndex,
    settings: &TasksSettings,
    previous_daily_path: Option<&Path>,
    inputs: &mut Vec<InputSnapshot>,
) -> ReconcileOutcome {
    let mut worker = ReconcileWorker::new(
        vault,
        files,
        note_index,
        settings,
        previous_daily_path,
    );
    worker.plan(files, inputs);
    worker.apply(files)
}

/// Key of a pending task-line rewrite: (file index, line index).
type TaskRewriteKey = (usize, usize);

/// A pending task-line rewrite: (text, id, depends).
type TaskRewrite = (String, Option<String>, Vec<String>);

/// One computed content edit: replace, remove, or insert a logical line.
struct PendingEdit {
    file_index: usize,
    line_index: usize,
    kind: EditKind,
    new_line: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Replace,
    Remove,
    Insert,
}

struct ReconcileWorker<'a> {
    vault: &'a Path,
    note_index: &'a NoteIndex,
    settings: &'a TasksSettings,
    previous_daily_path: Option<&'a Path>,
    /// (file index, task index) per scanned task identity.
    task_lookup: BTreeMap<(PathBuf, String), (usize, usize)>,
    /// Vault-wide `[id::]` per scanned task identity.
    id_lookup: BTreeMap<(PathBuf, String), Option<String>>,
    /// Block id to scanned tasks carrying it, built once per run so R3
    /// healing does not rescan the whole vault for every dependent.
    by_block: BTreeMap<String, Vec<(PathBuf, Option<String>)>>,
    /// Every `^block-id` defined on a non-task block, per note.
    non_task_blocks: BTreeMap<PathBuf, BTreeSet<String>>,
    /// Parsed `done/` targets, loaded on demand for archive links.
    archive_tasks: BTreeMap<PathBuf, Vec<TaskLine>>,
    /// File snapshots: lines plus per-task parse data.
    snapshots: Vec<NoteSnapshot>,
    /// Live graph for R7: dependent identity to scanned targets.
    graph: BTreeMap<(PathBuf, String), BTreeSet<(PathBuf, String)>>,
    /// Dependent note location per graph node.
    graph_nodes: BTreeMap<(PathBuf, String), (usize, usize)>,
    /// Pending task-line rewrites: (file, line) to (text, id, depends).
    task_rewrites: BTreeMap<TaskRewriteKey, TaskRewrite>,
    /// File index of the previous daily snapshot, which is never
    /// written (`contract` §4.1).
    previous_daily_file: Option<usize>,
    edits: Vec<PendingEdit>,
    outcome: ReconcileOutcome,
}

struct NoteSnapshot {
    lines: Vec<String>,
    fenced: BTreeSet<usize>,
    line_ending: &'static str,
    final_newline: bool,
    /// Task indices in this file, ascending by line.
    tasks: Vec<usize>,
}

/// Read-only task view used during planning.
struct TaskView<'a> {
    file_index: usize,
    relative_path: &'a Path,
    task: &'a TaskLine,
}

impl ReconcileOutcome {
    fn empty() -> Self {
        Self {
            touched_files: BTreeSet::new(),
            projection_updates: Vec::new(),
            adopted: Vec::new(),
            healed: Vec::new(),
            canonicalized: Vec::new(),
            legacy_children: 0,
            warnings: Vec::new(),
            archive_closed_ids: BTreeSet::new(),
            original_contents: BTreeMap::new(),
        }
    }
}

impl<'a> ReconcileWorker<'a> {
    fn new(
        vault: &'a Path,
        files: &[FileScan],
        note_index: &'a NoteIndex,
        settings: &'a TasksSettings,
        previous_daily_path: Option<&'a Path>,
    ) -> Self {
        let mut task_lookup = BTreeMap::new();
        let mut id_lookup = BTreeMap::new();
        for (file_index, file) in files.iter().enumerate() {
            for (task_index, task) in file.tasks.iter().enumerate() {
                if let Some(block_id) = &task.block_id {
                    let identity =
                        (file.relative_path.clone(), block_id.clone());
                    task_lookup
                        .entry(identity.clone())
                        .or_insert((file_index, task_index));
                    id_lookup.insert(identity, task.task_id.clone());
                }
            }
        }
        let mut non_task_blocks: BTreeMap<PathBuf, BTreeSet<String>> =
            BTreeMap::new();
        let mut snapshots = Vec::with_capacity(files.len());
        for file in files {
            let mut tasks = file
                .tasks
                .iter()
                .enumerate()
                .map(|(task_index, _)| task_index)
                .collect::<Vec<_>>();
            tasks.sort_by_key(|task_index| file.tasks[*task_index].line_index);
            let line_ending =
                task_dependencies::format::note_line_ending(&file.contents);
            let final_newline =
                task_dependencies::format::has_final_newline(&file.contents);
            // Notes with no tasks and no `^block-id` hold no Depends-On
            // line, dependency field, or target block: they get no line
            // views at all instead of an owned per-line copy.
            if tasks.is_empty() && !file.contents.contains('^') {
                snapshots.push(NoteSnapshot {
                    lines: Vec::new(),
                    fenced: BTreeSet::new(),
                    line_ending,
                    final_newline,
                    tasks,
                });
                continue;
            }
            // Each participating note's line views are computed once and
            // shared by the non-task block scan and the snapshot below.
            let lines = logical_lines(&file.contents);
            let fenced = fenced_lines(&lines, 0..lines.len());
            for (line_index, line) in lines.iter().enumerate() {
                if fenced.contains(&line_index) {
                    continue;
                }
                if let Some(block_id) = trailing_block_id(line) {
                    non_task_blocks
                        .entry(file.relative_path.clone())
                        .or_default()
                        .insert(block_id);
                }
            }
            if let Some(blocks) = non_task_blocks.get_mut(&file.relative_path) {
                for task in &file.tasks {
                    if let Some(block_id) = &task.block_id {
                        blocks.remove(block_id);
                    }
                }
            }
            // Owned per-line copies only for notes with tasks: the apply
            // step rebuilds those notes while mutating their contents, so
            // borrowed views cannot outlive planning. Task-less notes are
            // scanned transiently above and keep no copy.
            snapshots.push(if tasks.is_empty() {
                NoteSnapshot {
                    lines: Vec::new(),
                    fenced: BTreeSet::new(),
                    line_ending,
                    final_newline,
                    tasks,
                }
            } else {
                NoteSnapshot {
                    lines: lines.iter().map(|line| line.to_string()).collect(),
                    fenced,
                    line_ending,
                    final_newline,
                    tasks,
                }
            });
        }
        let previous_daily_file = previous_daily_path
            .and_then(|path| files.iter().position(|file| file.path == *path));
        let mut by_block: BTreeMap<String, Vec<(PathBuf, Option<String>)>> =
            BTreeMap::new();
        for ((path, block), id) in &id_lookup {
            by_block
                .entry(block.clone())
                .or_default()
                .push((path.clone(), id.clone()));
        }
        Self {
            vault,
            note_index,
            settings,
            previous_daily_path,
            task_lookup,
            id_lookup,
            by_block,
            non_task_blocks,
            archive_tasks: BTreeMap::new(),
            snapshots,
            graph: BTreeMap::new(),
            graph_nodes: BTreeMap::new(),
            task_rewrites: BTreeMap::new(),
            previous_daily_file,
            edits: Vec::new(),
            outcome: ReconcileOutcome::empty(),
        }
    }

    /// Plan every open dependent's reconciliation without mutating.
    fn plan(&mut self, files: &[FileScan], inputs: &mut Vec<InputSnapshot>) {
        let order: Vec<(usize, usize)> = files
            .iter()
            .enumerate()
            .flat_map(|(file_index, _)| {
                self.snapshots[file_index]
                    .tasks
                    .iter()
                    .map(move |task_index| (file_index, *task_index))
            })
            .collect();
        for (file_index, task_index) in order {
            if self
                .previous_daily_path
                .is_some_and(|path| files[file_index].path == *path)
            {
                continue;
            }
            let task = &files[file_index].tasks[task_index];
            if !task.status_recognized || !task.status_type.is_open() {
                continue;
            }
            // Closed dependents (DR18) are never touched; the borrow
            // ends before any mutation below.
            let view = TaskView {
                file_index,
                relative_path: &files[file_index].relative_path,
                task,
            };
            self.plan_dependent(&view, files, inputs);
        }
        self.plan_cycles(files);
    }

    /// Resolution of one Depends-On link or legacy child target.
    fn resolve_link_target(
        &mut self,
        source: &Path,
        own_block: Option<&str>,
        target: &str,
        block_id: &str,
        inputs: &mut Vec<InputSnapshot>,
    ) -> LinkTarget {
        let target = target.trim();
        let Some(resolved) = self.note_index.resolve(Some(source), target)
        else {
            if let Some(archive) =
                explicit_archive_reference_path(target.trim())
            {
                return self.resolve_archive_target(&archive, block_id, inputs);
            }
            return LinkTarget::Unresolved;
        };
        if task_dependencies::is_archive_path(&resolved) {
            return self.resolve_archive_target(&resolved, block_id, inputs);
        }
        if let Some(own) = own_block
            && resolved == source
            && block_id == own
        {
            return LinkTarget::SelfTarget;
        }
        let identity = (resolved.clone(), block_id.to_string());
        if self.task_lookup.contains_key(&identity) {
            return LinkTarget::Task {
                path: resolved,
                block: block_id.to_string(),
                id: self.id_lookup.get(&identity).cloned().flatten(),
            };
        }
        if self
            .non_task_blocks
            .get(&resolved)
            .is_some_and(|blocks| blocks.contains(block_id))
        {
            return LinkTarget::NonTask {
                path: resolved,
                block: block_id.to_string(),
            };
        }
        LinkTarget::Unresolved
    }

    /// Resolve a `done/` archive link through a directly loaded task
    /// catalog. Archive prerequisites are resolved, closed history:
    /// their id is kept, never warned about, never blocking.
    fn resolve_archive_target(
        &mut self,
        archive: &Path,
        block_id: &str,
        inputs: &mut Vec<InputSnapshot>,
    ) -> LinkTarget {
        let archive_path = archive.to_path_buf();
        if !self.archive_tasks.contains_key(&archive_path) {
            let path = self.vault.join(archive);
            let snapshot = match capture_optional(&path, InputKind::Archive) {
                Ok(snapshot) => snapshot,
                Err(_) => return LinkTarget::Unresolved,
            };
            let contents = match snapshot.utf8_contents() {
                Ok(Some(contents)) => contents,
                _ => {
                    push_unique_input(inputs, snapshot);
                    return LinkTarget::Unresolved;
                }
            };
            push_unique_input(inputs, snapshot);
            self.archive_tasks.insert(
                archive_path.clone(),
                parse_tasks(&contents, self.settings),
            );
        }
        let found = self.archive_tasks.get(&archive_path).and_then(|tasks| {
            tasks
                .iter()
                .find(|task| task.block_id.as_deref() == Some(block_id))
        });
        match found {
            Some(task) => LinkTarget::Archive {
                path: archive_path,
                block: block_id.to_string(),
                id: task.task_id.clone(),
            },
            None => LinkTarget::Unresolved,
        }
    }
}

/// What one Depends-On link (or legacy child) resolved to.
#[derive(Debug, Clone)]
enum LinkTarget {
    /// A scanned vault task.
    Task {
        path: PathBuf,
        block: String,
        id: Option<String>,
    },
    /// A `done/` archive task: closed history.
    Archive {
        path: PathBuf,
        block: String,
        id: Option<String>,
    },
    /// A block that exists but is not a task (R5).
    NonTask { path: PathBuf, block: String },
    /// The dependent's own task (R6).
    SelfTarget,
    /// No usable target (R4).
    Unresolved,
}

/// One link on a Depends-On line, resolved and ready to project.
struct PlannedLink {
    block: String,
    resolved: LinkTarget,
    /// Original token text (`!`, `~~`, alias preserved) for R4 keeps.
    verbatim: String,
    /// Rewritten link text (shortest form, or verbatim for keeps).
    new_text: String,
    healed: bool,
}

impl ReconcileWorker<'_> {
    /// Reconcile one open dependent: adopt, heal, canonicalise, warn.
    fn plan_dependent(
        &mut self,
        view: &TaskView<'_>,
        files: &[FileScan],
        inputs: &mut Vec<InputSnapshot>,
    ) {
        let snap_lines: Vec<&str> = self.snapshots[view.file_index]
            .lines
            .iter()
            .map(String::as_str)
            .collect();
        let child = task_dependencies::parse::dependency_child_of(
            &snap_lines,
            &self.snapshots[view.file_index].fenced,
            view.task.line_index,
        );
        let child_index = child.as_ref().map(|child| child.line_index);
        let legacy = self.plan_legacy(view, child_index, inputs);
        match child.map(|child| child.parsed) {
            // `dependency_child_of` never returns `NotALine`, but the
            // type says it can; both mean "no managed line" (R2).
            None | Some(task_dependencies::DependencyLine::NotALine) => {
                self.plan_adoption(view, &legacy, None);
            }
            Some(task_dependencies::DependencyLine::Malformed) => {
                self.warn(
                    view,
                    "malformed_dependency_line",
                    "Depends-On line has trailing prose, a half-typed link, or other residue; left alone",
                );
            }
            Some(task_dependencies::DependencyLine::Empty) => {
                let Some(child_index) = child_index else {
                    return;
                };
                self.plan_empty(view, &legacy, child_index);
            }
            Some(task_dependencies::DependencyLine::Accepted {
                links,
                canonical,
            }) => {
                let Some(child_index) = child_index else {
                    return;
                };
                self.plan_accepted(
                    view,
                    files,
                    inputs,
                    &legacy,
                    child_index,
                    links,
                    canonical,
                );
            }
        }
    }
}

impl LinkTarget {
    /// Vault identity of the prerequisite block.
    fn block_identity(&self) -> Option<(PathBuf, String)> {
        match self {
            Self::Task { path, block, .. }
            | Self::Archive { path, block, .. }
            | Self::NonTask { path, block } => {
                Some((path.clone(), block.clone()))
            }
            Self::SelfTarget | Self::Unresolved => None,
        }
    }
}

/// One R8 legacy child that counts as a dep link: its resolved target
/// plus the vault-wide id the field matched.
struct LegacyMember {
    path: PathBuf,
    block: String,
    id: String,
    archive: bool,
}

impl ReconcileWorker<'_> {
    /// Collect the dependent's R8 legacy children: direct child
    /// bullets whose only content is one block link and whose target
    /// id is managed by the dependent's field. Never rewritten here;
    /// the vault migration folds them into the line.
    fn plan_legacy(
        &mut self,
        view: &TaskView<'_>,
        dependency_line: Option<usize>,
        inputs: &mut Vec<InputSnapshot>,
    ) -> Vec<LegacyMember> {
        let mut members = Vec::new();
        if view.task.depends_on.is_empty() {
            return members;
        }
        // Two passes: collect candidate child links while borrowing
        // the snapshot, then resolve (which mutates the archive cache).
        let candidates = {
            let snap = &self.snapshots[view.file_index];
            let refs: Vec<&str> =
                snap.lines.iter().map(String::as_str).collect();
            let source_indent =
                leading_indentation_width(refs[view.task.line_index]);
            let mut candidates = Vec::new();
            for line_index in view.task.line_index + 1..refs.len() {
                if dependency_line == Some(line_index) {
                    continue;
                }
                let line = refs[line_index];
                if line.trim().is_empty() {
                    continue;
                }
                if snap.fenced.contains(&line_index) {
                    continue;
                }
                if leading_indentation_width(line) <= source_indent {
                    break;
                }
                if nearest_parent_list_item(&refs, line_index)
                    != Some(view.task.line_index)
                {
                    continue;
                }
                let Some(legacy) =
                    task_dependencies::legacy_child_reference(line)
                else {
                    continue;
                };
                // Explicit `done/` targets resolve through the archive
                // catalog below and follow §4.3 like any other legacy
                // child; they are never skipped here.
                candidates.push((legacy.target, legacy.block_id));
            }
            candidates
        };
        for (target, block_id) in candidates {
            let resolved = self.resolve_link_target(
                view.relative_path,
                view.task.block_id.as_deref(),
                &target,
                &block_id,
                inputs,
            );
            match resolved {
                LinkTarget::Task {
                    path, block, id, ..
                } => {
                    let Some(matched) =
                        self.legacy_cover(view, &path, &block, id.as_deref())
                    else {
                        continue;
                    };
                    members.push(LegacyMember {
                        path,
                        block,
                        id: matched,
                        archive: false,
                    });
                }
                LinkTarget::Archive { path, block, id } => {
                    let matched = match id {
                        Some(id)
                            if view
                                .task
                                .depends_on
                                .iter()
                                .any(|dependency| dependency == &id) =>
                        {
                            Some(id)
                        }
                        _ => task_dependencies::dependency_id(&path, &block)
                            .ok()
                            .filter(|canonical| {
                                view.task
                                    .depends_on
                                    .iter()
                                    .any(|dependency| dependency == canonical)
                            }),
                    };
                    let Some(matched) = matched else {
                        continue;
                    };
                    members.push(LegacyMember {
                        path,
                        block,
                        id: matched,
                        archive: true,
                    });
                }
                LinkTarget::NonTask { .. }
                | LinkTarget::SelfTarget
                | LinkTarget::Unresolved => continue,
            }
        }
        members
    }

    /// The field id covering a scanned legacy target: its `[id::]`,
    /// canonical id, or same-note bare block id — or `None` when the
    /// field manages none of them and the child is content, not an
    /// edge.
    fn legacy_cover(
        &self,
        view: &TaskView<'_>,
        target_path: &Path,
        block_id: &str,
        task_id: Option<&str>,
    ) -> Option<String> {
        if let Some(task_id) = task_id
            && view
                .task
                .depends_on
                .iter()
                .any(|dependency| dependency == task_id)
        {
            return Some(task_id.to_string());
        }
        if let Ok(canonical) =
            task_dependencies::dependency_id(target_path, block_id)
            && view
                .task
                .depends_on
                .iter()
                .any(|dependency| dependency == &canonical)
        {
            return Some(canonical);
        }
        (target_path == view.relative_path
            && view
                .task
                .depends_on
                .iter()
                .any(|dependency| dependency == block_id))
        .then(|| block_id.to_string())
    }

    /// R1 with healing and canonicalisation: resolve every line link,
    /// heal what R3 covers, project the field, and rewrite the line
    /// when it is healed or non-canonical.
    #[allow(clippy::too_many_arguments)]
    fn plan_accepted(
        &mut self,
        view: &TaskView<'_>,
        files: &[FileScan],
        inputs: &mut Vec<InputSnapshot>,
        legacy: &[LegacyMember],
        child_index: usize,
        links: Vec<task_dependencies::parse::DependencyLink>,
        canonical: bool,
    ) {
        let child_line =
            self.snapshots[view.file_index].lines[child_index].clone();
        let spans = task_dependencies::block_link_spans(&child_line);
        let mut planned = links
            .iter()
            .enumerate()
            .map(|(index, link)| {
                let verbatim = spans.get(index).map_or_else(
                    || format!("[[{}#^{}]]", link.target, link.block_id),
                    |span| {
                        child_line[span.full_start..span.full_end].to_string()
                    },
                );
                let resolved = self.resolve_link_target(
                    view.relative_path,
                    view.task.block_id.as_deref(),
                    &link.target,
                    &link.block_id,
                    inputs,
                );
                PlannedLink {
                    block: link.block_id.clone(),
                    resolved,
                    verbatim,
                    new_text: String::new(),
                    healed: false,
                }
            })
            .collect::<Vec<_>>();
        // Warn R5/R6 up front: those links are kept verbatim and never
        // projected.
        for link in &planned {
            match &link.resolved {
                LinkTarget::NonTask { path, block } => self.warn(
                    view,
                    "non_task_dependency",
                    &format!(
                        "link to {}#^{} resolves to a non-task block; kept, not projected",
                        display_path(path),
                        block
                    ),
                ),
                LinkTarget::SelfTarget => self.warn(
                    view,
                    "self_dependency",
                    "link points to its own task; kept, not projected",
                ),
                LinkTarget::Task { .. }
                | LinkTarget::Archive { .. }
                | LinkTarget::Unresolved => {}
            }
        }
        self.heal_links(view, &mut planned);
        let unresolved = planned
            .iter()
            .any(|link| matches!(link.resolved, LinkTarget::Unresolved));
        if unresolved {
            for link in planned
                .iter()
                .filter(|link| matches!(link.resolved, LinkTarget::Unresolved))
            {
                self.warn(
                    view,
                    "unresolved_dependency_link",
                    &format!(
                        "link {} kept verbatim; it never blocks",
                        link.verbatim
                    ),
                );
            }
        }
        let mut set: Vec<(PathBuf, String, String, bool)> = Vec::new();
        let mut seen = BTreeSet::new();
        let mut line_links: Vec<String> = Vec::new();
        for link in &mut planned {
            // A resolved target that cannot project (a previous-daily
            // or unencodable target with no `[id::]`) keeps its link
            // verbatim, like R4–R6 keeps.
            let mut keep_verbatim = false;
            match &link.resolved {
                LinkTarget::Task { id, .. } => {
                    let identity = link
                        .resolved
                        .block_identity()
                        .expect("task target has an identity");
                    if seen.insert(identity.clone()) {
                        match self.project_id(
                            view,
                            files,
                            &identity,
                            id.clone(),
                        ) {
                            Some(id) => {
                                set.push((identity.0, identity.1, id, false));
                            }
                            None => keep_verbatim = true,
                        }
                    }
                }
                LinkTarget::Archive { id, .. } => {
                    let identity = link
                        .resolved
                        .block_identity()
                        .expect("archive target has an identity");
                    if seen.insert(identity.clone())
                        && let Some(id) = self.archive_id(&identity, id.clone())
                    {
                        self.outcome.archive_closed_ids.insert(id.clone());
                        set.push((identity.0, identity.1, id, true));
                    }
                }
                LinkTarget::NonTask { .. }
                | LinkTarget::SelfTarget
                | LinkTarget::Unresolved => {}
            }
            let text = match &link.resolved {
                LinkTarget::Task { .. } if !keep_verbatim => {
                    let identity = link
                        .resolved
                        .block_identity()
                        .expect("resolved target has an identity");
                    self.shortest_link(view, &identity)
                }
                LinkTarget::Archive { .. } => {
                    let identity = link
                        .resolved
                        .block_identity()
                        .expect("resolved target has an identity");
                    self.shortest_link(view, &identity)
                }
                LinkTarget::Task { .. }
                | LinkTarget::NonTask { .. }
                | LinkTarget::SelfTarget
                | LinkTarget::Unresolved => link.verbatim.clone(),
            };
            link.new_text = text.clone();
            line_links.push(text);
        }
        for member in legacy {
            let identity = (member.path.clone(), member.block.clone());
            if seen.insert(identity.clone()) {
                if member.archive {
                    self.outcome.archive_closed_ids.insert(member.id.clone());
                }
                set.push((
                    identity.0,
                    identity.1,
                    member.id.clone(),
                    member.archive,
                ));
            }
        }
        self.outcome.legacy_children += legacy.len();
        self.record_graph(view, &set);
        self.project_field(view, &set, unresolved);
        let healed_any = planned.iter().any(|link| link.healed);
        let rebuilt = rebuild_child_line(&child_line, &line_links);
        if healed_any || !canonical || rebuilt != child_line {
            self.replace_line(view.file_index, child_index, &rebuilt);
            if healed_any {
                for link in planned.iter().filter(|link| link.healed) {
                    self.outcome.healed.push(DependencyLineReport {
                        path: display_path(view.relative_path),
                        line: child_index + 1,
                        detail: format!(
                            "{} -> {}",
                            link.verbatim, link.new_text
                        ),
                    });
                }
            }
            if rebuilt != child_line {
                self.outcome.canonicalized.push(DependencyLineReport {
                    path: display_path(view.relative_path),
                    line: child_index + 1,
                    detail: rebuilt,
                });
            }
        }
    }

    /// R3: rewrite an unresolvable link when exactly one scanned task
    /// carries both the link's block id and one of the dependent's
    /// unaccounted field ids. The healed link then flows through R1.
    fn heal_links(&mut self, view: &TaskView<'_>, planned: &mut [PlannedLink]) {
        let mut resolved_ids = BTreeSet::new();
        for link in planned.iter() {
            match &link.resolved {
                LinkTarget::Task { id, .. } => {
                    let identity = link
                        .resolved
                        .block_identity()
                        .expect("task target has an identity");
                    resolved_ids.insert(id.clone().unwrap_or_else(|| {
                        task_dependencies::dependency_id(
                            &identity.0,
                            &identity.1,
                        )
                        .unwrap_or_default()
                    }));
                }
                LinkTarget::Archive { id, .. } => {
                    let identity = link
                        .resolved
                        .block_identity()
                        .expect("archive target has an identity");
                    resolved_ids.insert(id.clone().unwrap_or_else(|| {
                        task_dependencies::dependency_id(
                            &identity.0,
                            &identity.1,
                        )
                        .unwrap_or_default()
                    }));
                }
                LinkTarget::NonTask { .. }
                | LinkTarget::SelfTarget
                | LinkTarget::Unresolved => {}
            }
        }
        let mut unaccounted: Vec<String> = view
            .task
            .depends_on
            .iter()
            .filter(|id| !resolved_ids.contains(*id))
            .cloned()
            .collect();
        for link in planned.iter_mut() {
            if !matches!(link.resolved, LinkTarget::Unresolved) {
                continue;
            }
            let candidates = self
                .by_block
                .get(&link.block)
                .map(|tasks| {
                    tasks
                        .iter()
                        .filter(|(_, id)| {
                            id.as_ref().is_some_and(|id| {
                                unaccounted.iter().any(|left| left == id)
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if candidates.len() != 1 {
                continue;
            }
            let (path, id) = candidates[0].clone();
            let id = id.expect("heal candidate carries an id");
            let block = link.block.clone();
            link.resolved = LinkTarget::Task {
                path: path.clone(),
                block: block.clone(),
                id: Some(id.clone()),
            };
            if let Some(position) =
                unaccounted.iter().position(|left| *left == id)
            {
                unaccounted.remove(position);
            }
            link.healed = true;
        }
    }

    /// The vault-wide id a resolved scanned target projects: its
    /// existing `[id::]`, never rewritten, or the canonical id —
    /// stamping the target when it has none (R1 `target_id`). A target
    /// in the previous daily snapshot is never stamped (`contract`
    /// §4.1, `previous_daily_target`), and a target whose path cannot
    /// encode without an `[id::]` warns (`unencodable_dependency_target`,
    /// `contract` §3); both stay unprojected.
    fn project_id(
        &mut self,
        view: &TaskView<'_>,
        files: &[FileScan],
        identity: &(PathBuf, String),
        id: Option<String>,
    ) -> Option<String> {
        if let Some(id) = id {
            return Some(id);
        }
        let (file_index, task_index) =
            self.task_lookup.get(identity).copied()?;
        if self.previous_daily_file == Some(file_index) {
            self.warn(
                view,
                "previous_daily_target",
                &format!(
                    "target {}#^{} lives in the previous daily note; kept, never stamped or projected",
                    display_path(&identity.0),
                    identity.1
                ),
            );
            return None;
        }
        let canonical = match task_dependencies::dependency_id(
            &identity.0,
            &identity.1,
        ) {
            Ok(canonical) => canonical,
            Err(_) => {
                self.warn(
                        view,
                        "unencodable_dependency_target",
                        &format!(
                            "target {}#^{} has no [id::] and its path cannot encode; kept, never projected",
                            display_path(&identity.0),
                            identity.1
                        ),
                    );
                return None;
            }
        };
        let target_line_index = files[file_index].tasks[task_index].line_index;
        let depends = self.pending_or_scanned_depends(
            file_index,
            target_line_index,
            files[file_index].tasks[task_index].depends_on.clone(),
        );
        let current =
            self.pending_or_snapshot_line(file_index, target_line_index);
        let stamped = set_task_fields(&current, Some(&canonical), &depends);
        if stamped != current {
            self.replace_task_line(
                view,
                file_index,
                target_line_index,
                &stamped,
                Some(canonical.clone()),
                depends,
            );
            self.outcome
                .projection_updates
                .push(DependencyProjectionUpdate {
                    kind: "target_id".to_string(),
                    path: display_path(&identity.0),
                    line: target_line_index + 1,
                    detail: format!("[id:: {canonical}]"),
                });
        }
        Some(canonical)
    }

    /// The vault-wide id an archive target projects: its `[id::]` or
    /// the canonical id. Archive notes are history and are never
    /// stamped; without any id the target cannot project.
    fn archive_id(
        &self,
        identity: &(PathBuf, String),
        id: Option<String>,
    ) -> Option<String> {
        if let Some(id) = id {
            return Some(id);
        }
        task_dependencies::dependency_id(&identity.0, &identity.1).ok()
    }

    /// Shortest unambiguous link text for a target (`contract` §3),
    /// except archive targets keep the explicit `done/` path: `done/`
    /// notes live outside the hooks' note index, so only the full
    /// path re-resolves on the next run.
    fn shortest_link(
        &self,
        view: &TaskView<'_>,
        identity: &(PathBuf, String),
    ) -> String {
        if task_dependencies::is_archive_path(&identity.0) {
            let mut without_extension = identity.0.clone();
            without_extension.set_extension("");
            let text = without_extension
                .components()
                .filter_map(|component| component.as_os_str().to_str())
                .collect::<Vec<_>>()
                .join("/");
            return format!("[[{text}#^{}]]", identity.1);
        }
        task_dependencies::format::canonical_link(
            &identity.0,
            &identity.1,
            view.relative_path,
            self.note_index,
        )
    }

    /// R1 projection: the dependent's field becomes the set's ids in
    /// set order. Stale ids are dropped and reported — except while an
    /// unhealable link remains, when unaccounted ids stay on as heal
    /// breadcrumbs (R4).
    fn project_field(
        &mut self,
        view: &TaskView<'_>,
        set: &[(PathBuf, String, String, bool)],
        unresolved: bool,
    ) {
        let projected: Vec<String> =
            set.iter().map(|(_, _, id, _)| id.clone()).collect();
        let in_set: BTreeSet<&str> =
            projected.iter().map(String::as_str).collect();
        let unaccounted: Vec<String> = view
            .task
            .depends_on
            .iter()
            .filter(|id| !in_set.contains(id.as_str()))
            .cloned()
            .collect();
        let (kept, dropped) = if unresolved {
            (unaccounted.clone(), Vec::new())
        } else {
            (Vec::new(), unaccounted.clone())
        };
        let mut next = projected.clone();
        next.extend(kept.iter().cloned());
        if next == view.task.depends_on {
            return;
        }
        if !dropped.is_empty() {
            self.warn(
                view,
                "dependency_field_ids_dropped",
                &format!(
                    "dropped unaccounted field ids {}",
                    dropped.join(", ")
                ),
            );
        }
        let mut detail = format!("dependsOn := {}", next.join(", "));
        if !dropped.is_empty() {
            detail.push_str(&format!("; dropped {}", dropped.join(", ")));
        }
        if !kept.is_empty() {
            detail.push_str(&format!("; heal breadcrumbs {}", kept.join(", ")));
        }
        self.write_dependent_field(view, next, detail);
    }

    /// R9: a label-only line is the source of truth — with no legacy
    /// children behind it the line is deleted and the field goes with
    /// it, whatever the field holds. Adoptable ids are never
    /// re-adopted onto the line (DW5/DR16). Legacy children are never
    /// folded into the line here; they follow R8/R1 through the
    /// adoption path below, and the vault migration owns that write.
    fn plan_empty(
        &mut self,
        view: &TaskView<'_>,
        legacy: &[LegacyMember],
        child_index: usize,
    ) {
        if legacy.is_empty() {
            self.remove_label_only_line(view, child_index);
            return;
        }
        self.plan_adoption(view, legacy, Some(child_index));
    }

    /// Report a deleted label-only Depends-On line.
    fn push_line_removed(&mut self, view: &TaskView<'_>, child_index: usize) {
        self.outcome
            .projection_updates
            .push(DependencyProjectionUpdate {
                kind: "line_removed".to_string(),
                path: display_path(view.relative_path),
                line: child_index + 1,
                detail: "empty Depends-On line removed".to_string(),
            });
    }

    /// R9: delete a label-only line and remove the field with it,
    /// whatever the field holds. The dropped ids warn once as
    /// `dependency_field_ids_dropped` and report as `line_removed`
    /// plus `dependsOn removed`; an already-empty field needs no
    /// rewrite.
    fn remove_label_only_line(
        &mut self,
        view: &TaskView<'_>,
        child_index: usize,
    ) {
        self.remove_line(view.file_index, child_index);
        self.push_line_removed(view, child_index);
        if view.task.depends_on.is_empty() {
            return;
        }
        let dropped = view.task.depends_on.clone();
        self.warn(
            view,
            "dependency_field_ids_dropped",
            &format!("dropped unaccounted field ids {}", dropped.join(", ")),
        );
        self.write_dependent_field(
            view,
            Vec::new(),
            "dependsOn removed".to_string(),
        );
    }

    /// Rewrite the dependent's `[dependsOn::]` to `next` and report it
    /// as a `dependent_field` projection update carrying `detail`.
    fn write_dependent_field(
        &mut self,
        view: &TaskView<'_>,
        next: Vec<String>,
        detail: String,
    ) {
        let id = self.pending_or_scanned_id(
            view.file_index,
            view.task.line_index,
            view.task.task_id.clone(),
        );
        let current = self
            .pending_or_snapshot_line(view.file_index, view.task.line_index);
        let updated = set_task_fields(&current, id.as_deref(), &next);
        self.replace_task_line(
            view,
            view.file_index,
            view.task.line_index,
            &updated,
            id,
            next,
        );
        self.outcome
            .projection_updates
            .push(DependencyProjectionUpdate {
                kind: "dependent_field".to_string(),
                path: display_path(view.relative_path),
                line: view.task.line_index + 1,
                detail,
            });
    }

    /// Warn for ids that stay in the field: self-naming ids (R6) are
    /// never adopted, and ids matching no task with a block id (R2)
    /// stay until their target appears.
    fn warn_kept_ids(
        &mut self,
        view: &TaskView<'_>,
        self_named: &[String],
        unadoptable: &[String],
    ) {
        for id in self_named {
            self.warn(
                view,
                "self_dependency",
                &format!(
                    "field id {id} names its own task; kept, not projected"
                ),
            );
        }
        for id in unadoptable {
            self.warn(
                view,
                "unadoptable_dependency_id",
                &format!(
                    "field id {id} matches no task with a block id; kept in the field"
                ),
            );
        }
    }

    /// R2: without a line, adopt field ids not covered by legacy
    /// children — write one canonical line for the adoptable ones. Ids
    /// with no `^block-id` anywhere stay in the field with a warning.
    /// Never infer a removal from a missing line.
    fn plan_adoption(
        &mut self,
        view: &TaskView<'_>,
        legacy: &[LegacyMember],
        empty_line: Option<usize>,
    ) {
        self.outcome.legacy_children += legacy.len();
        let covered: BTreeSet<&str> =
            legacy.iter().map(|member| member.id.as_str()).collect();
        let uncovered: Vec<String> = view
            .task
            .depends_on
            .iter()
            .filter(|id| !covered.contains(id.as_str()))
            .cloned()
            .collect();
        if uncovered.is_empty() {
            if let Some(child_index) = empty_line {
                // Label-only line, nothing behind it: R9 deletes the
                // line. Every field id is covered here, so a remaining
                // field still covers legacy children and stays; an empty
                // field needs no rewrite.
                self.remove_line(view.file_index, child_index);
                self.push_line_removed(view, child_index);
            }
            let set: Vec<(PathBuf, String, String, bool)> = legacy
                .iter()
                .map(|member| {
                    (
                        member.path.clone(),
                        member.block.clone(),
                        member.id.clone(),
                        member.archive,
                    )
                })
                .collect();
            self.record_graph(view, &set);
            return;
        }
        // Vault tasks carrying each uncovered id that have a block id.
        let mut by_id: BTreeMap<String, Vec<(PathBuf, String)>> =
            BTreeMap::new();
        for ((path, block), id) in &self.id_lookup {
            if let Some(id) = id
                && uncovered.iter().any(|left| left == id)
            {
                by_id
                    .entry(id.clone())
                    .or_default()
                    .push((path.clone(), block.clone()));
            }
        }
        // Ids naming the dependent itself (R6) are never adopted:
        // adopting them would write a self link the next run drops,
        // churning forever. They stay in the field with a warning.
        let mut own_ids = BTreeSet::new();
        if let Some(own) = &view.task.task_id {
            own_ids.insert(own.clone());
        }
        if let Some(own_block) = &view.task.block_id
            && let Ok(canonical) =
                task_dependencies::dependency_id(view.relative_path, own_block)
        {
            own_ids.insert(canonical);
        }
        let mut adopted: Vec<(PathBuf, String, String)> = Vec::new();
        let mut adoptable_seen = BTreeSet::new();
        // Ids that stay in the field warn only on paths that keep the
        // field; the R9 label-only drop below reports its own removal
        // instead, so collection happens here and warnings emit later.
        let mut unadoptable: Vec<String> = Vec::new();
        let mut self_named: Vec<String> = Vec::new();
        for id in &uncovered {
            if own_ids.contains(id) {
                self_named.push(id.clone());
                continue;
            }
            let Some(targets) = by_id.get(id) else {
                unadoptable.push(id.clone());
                continue;
            };
            if !adoptable_seen.insert(id.clone()) {
                continue;
            }
            // Deterministic choice: first in vault scan order.
            let (path, block) = targets[0].clone();
            adopted.push((path, block, id.clone()));
        }
        // Set order mirrors the line the adoption writes (adopted
        // links first) followed by legacy children, per the §4.2 set
        // definition.
        let set: Vec<(PathBuf, String, String, bool)> = adopted
            .iter()
            .map(|(path, block, id)| {
                (path.clone(), block.clone(), id.clone(), false)
            })
            .chain(legacy.iter().map(|member| {
                (
                    member.path.clone(),
                    member.block.clone(),
                    member.id.clone(),
                    member.archive,
                )
            }))
            .collect();
        self.record_graph(view, &set);
        // Unadoptable ids stay in the field (R2); the projected field
        // keeps field order for everything the set does not cover.
        let in_set: BTreeSet<&str> =
            set.iter().map(|(_, _, id, _)| id.as_str()).collect();
        let mut next: Vec<String> =
            set.iter().map(|(_, _, id, _)| id.clone()).collect();
        for id in &uncovered {
            if !in_set.contains(id.as_str()) {
                next.push(id.clone());
            }
        }
        if adopted.is_empty() {
            if let Some(child_index) = empty_line {
                // Nothing adoptable behind a label-only line: R9 deletes
                // the line (DW5/DR16), reported as `line_removed`.
                // `plan_empty` already handled the no-legacy-children case
                // (the field goes with the line there); legacy coverage
                // here keeps the field (R2).
                self.remove_line(view.file_index, child_index);
                self.push_line_removed(view, child_index);
            }
            // R2 keep path: ids staying in the field warn here, since the
            // R9 drop above reports its own removal instead.
            self.warn_kept_ids(view, &self_named, &unadoptable);
            if next != view.task.depends_on {
                let detail = format!("dependsOn := {}", next.join(", "));
                self.write_dependent_field(view, next.clone(), detail);
            }
            return;
        }
        let texts: Vec<String> = adopted
            .iter()
            .map(|(path, block, _)| {
                self.shortest_link(view, &(path.clone(), block.clone()))
            })
            .collect();
        // Adopted alongside ids that stay in the field (R2): those
        // warn here, since collection above no longer warns inline.
        self.warn_kept_ids(view, &self_named, &unadoptable);
        let (position, indent) = self.child_slot(view, empty_line);
        let line =
            task_dependencies::format::format_dependency_line(&indent, &texts);
        if let Some(child_index) = empty_line {
            self.replace_line(view.file_index, child_index, &line);
            self.outcome.adopted.push(DependencyLineReport {
                path: display_path(view.relative_path),
                line: child_index + 1,
                detail: line,
            });
        } else {
            self.insert_line(view.file_index, position, &line);
            self.outcome.adopted.push(DependencyLineReport {
                path: display_path(view.relative_path),
                line: position + 1,
                detail: line,
            });
        }
        if next != view.task.depends_on {
            let detail = format!("dependsOn := {}", next.join(", "));
            self.write_dependent_field(view, next.clone(), detail);
        }
    }

    /// Record the dependent's live graph edges for R7 cycle detection.
    /// Only scanned tasks join the graph; archive history, non-tasks,
    /// self links, and unresolved links never do.
    fn record_graph(
        &mut self,
        view: &TaskView<'_>,
        set: &[(PathBuf, String, String, bool)],
    ) {
        let Some(own) = view.task.block_id.clone() else {
            return;
        };
        let source = (view.relative_path.to_path_buf(), own);
        let targets: BTreeSet<(PathBuf, String)> = set
            .iter()
            .filter(|(_, _, _, archive)| !archive)
            .map(|(path, block, _, _)| (path.clone(), block.clone()))
            .filter(|target| target != &source)
            .collect();
        if targets.is_empty() {
            return;
        }
        self.graph_nodes
            .insert(source.clone(), (view.file_index, view.task.line_index));
        self.graph.entry(source).or_default().extend(targets);
    }

    /// R7: every dependency cycle stays Blocked and warns with its
    /// path. One warning per member set, rooted at the smallest
    /// member for determinism.
    fn plan_cycles(&mut self, files: &[FileScan]) {
        let mut seen_sets = BTreeSet::new();
        let starts: Vec<(PathBuf, String)> =
            self.graph.keys().cloned().collect();
        for start in starts {
            let mut stack = vec![start.clone()];
            let mut in_stack = BTreeSet::from([start.clone()]);
            self.walk_cycles(
                files,
                &start,
                &mut stack,
                &mut in_stack,
                &mut seen_sets,
            );
        }
    }

    fn walk_cycles(
        &mut self,
        files: &[FileScan],
        start: &(PathBuf, String),
        stack: &mut Vec<(PathBuf, String)>,
        in_stack: &mut BTreeSet<(PathBuf, String)>,
        seen_sets: &mut BTreeSet<Vec<(PathBuf, String)>>,
    ) {
        let current = stack.last().cloned().expect("non-empty walk stack");
        let targets = self.graph.get(&current).cloned().unwrap_or_default();
        for target in targets {
            if target == *start {
                let mut members = stack.clone();
                members.sort();
                if seen_sets.insert(members) {
                    self.warn_cycle(files, start, stack);
                }
                continue;
            }
            if in_stack.contains(&target) || !self.graph.contains_key(&target) {
                continue;
            }
            in_stack.insert(target.clone());
            stack.push(target.clone());
            self.walk_cycles(files, start, stack, in_stack, seen_sets);
            stack.pop();
            in_stack.remove(&target);
        }
    }
}

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

use super::super::project_dependencies::ProjectTree;
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
    pub(super) warnings: Vec<DependencyWarning>,
    /// Vault-wide ids of archive (`done/`) prerequisites: kept in the
    /// field, never warned about, never blocking.
    pub(super) archive_closed_ids: BTreeSet<String>,
    /// Original note contents for touched files only, so callers clone
    /// just what reconcile rewrote instead of the whole vault.
    pub(super) original_contents: BTreeMap<usize, String>,
    /// Accepted explicit edges from R1–R10, keyed by dependent
    /// `(file_index, task_index)` so id-less dependents are included
    /// (`docs/task-dependencies.md` §14.3, §14.6). Recorded before
    /// `add_resident_implicit_edges`.
    pub(super) explicit_edges:
        BTreeMap<(usize, usize), BTreeSet<(PathBuf, String)>>,
    /// Open dependents whose Depends-On line is malformed (R10). BLOCKS
    /// never newly lists them and never prunes existing entries that
    /// name them.
    pub(super) uncertain_dependents: BTreeSet<(usize, usize)>,
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
    project_tree: &ProjectTree,
    settings: &TasksSettings,
    previous_daily_path: Option<&Path>,
    inputs: &mut Vec<InputSnapshot>,
) -> ReconcileOutcome {
    let mut worker = ReconcileWorker::new(
        vault,
        files,
        note_index,
        project_tree,
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
    project_tree: &'a ProjectTree,
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
    /// Internal project warnings emitted once per dependent and field id.
    warned_project_internal: BTreeSet<(PathBuf, usize, String)>,
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
    task_index: usize,
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
            warnings: Vec::new(),
            archive_closed_ids: BTreeSet::new(),
            original_contents: BTreeMap::new(),
            explicit_edges: BTreeMap::new(),
            uncertain_dependents: BTreeSet::new(),
        }
    }
}

impl<'a> ReconcileWorker<'a> {
    fn new(
        vault: &'a Path,
        files: &[FileScan],
        note_index: &'a NoteIndex,
        project_tree: &'a ProjectTree,
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
            project_tree,
            settings,
            previous_daily_path,
            task_lookup,
            id_lookup,
            by_block,
            non_task_blocks,
            archive_tasks: BTreeMap::new(),
            snapshots,
            graph: BTreeMap::new(),
            warned_project_internal: BTreeSet::new(),
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
                task_index,
                relative_path: &files[file_index].relative_path,
                task,
            };
            self.plan_dependent(&view, files, inputs);
        }
        self.add_resident_implicit_edges(files);
        self.plan_cycles(files);
        self.warn_project_parent_cycles(files);
    }

    /// Project tasks and residents inherit every ancestor project's
    /// projected prerequisites for R7 cycle detection (PDS11). Internal
    /// targets have already been kept off the graph by R1–R10.
    fn add_resident_implicit_edges(&mut self, files: &[FileScan]) {
        for (file_index, file) in files.iter().enumerate() {
            let Some(project_task_index) =
                self.project_tree.project_task_index(file_index)
            else {
                continue;
            };
            let project_source = file
                .tasks
                .get(project_task_index)
                .and_then(|project_task| project_task.block_id.as_ref())
                .map(|block_id| (file.relative_path.clone(), block_id.clone()));
            let mut sources = Vec::new();
            if let Some(project_source) = &project_source
                && let Some(project_task) = file.tasks.get(project_task_index)
            {
                sources.push((project_source.clone(), project_task.line_index));
            }
            for task in &file.tasks {
                if !self.project_tree.is_resident(file_index, task) {
                    continue;
                }
                let Some(block_id) = &task.block_id else {
                    continue;
                };
                sources.push((
                    (file.relative_path.clone(), block_id.clone()),
                    task.line_index,
                ));
            }
            let owners = std::iter::once(file_index)
                .chain(self.project_tree.ancestors(file_index).iter().copied());
            for owner_index in owners {
                let owner = &files[owner_index];
                let targets = self
                    .project_tree
                    .project_task_index(owner_index)
                    .and_then(|task_index| owner.tasks.get(task_index))
                    .and_then(|task| task.block_id.as_ref())
                    .and_then(|block_id| {
                        self.graph.get(&(
                            owner.relative_path.clone(),
                            block_id.clone(),
                        ))
                    })
                    .cloned()
                    .unwrap_or_default();
                for (source, line_index) in &sources {
                    if owner_index == file_index
                        && project_source.as_ref() == Some(source)
                    {
                        continue;
                    }
                    let targets = targets
                        .iter()
                        .filter(|target| *target != source)
                        .cloned()
                        .collect::<BTreeSet<_>>();
                    if targets.is_empty() {
                        continue;
                    }
                    self.graph_nodes
                        .entry(source.clone())
                        .or_insert((file_index, *line_index));
                    self.graph
                        .entry(source.clone())
                        .or_default()
                        .extend(targets);
                }
            }
        }
    }

    fn warn_project_parent_cycles(&mut self, files: &[FileScan]) {
        let cycles = self.project_tree.cycles().to_vec();
        for cycle in cycles {
            let names = cycle
                .iter()
                .map(|index| display_path(&files[*index].relative_path))
                .collect::<Vec<_>>();
            let Some(file_index) = cycle.first().copied() else {
                continue;
            };
            let Some(task_index) =
                self.project_tree.project_task_index(file_index)
            else {
                continue;
            };
            let Some(task) = files[file_index].tasks.get(task_index) else {
                continue;
            };
            let view = TaskView {
                file_index,
                task_index,
                relative_path: &files[file_index].relative_path,
                task,
            };
            self.warn(
                &view,
                "project_parent_cycle",
                &format!("project parent cycle includes {}", names.join(" → ")),
            );
        }
    }

    /// Resolution of one Depends-On line link target.
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

/// What one Depends-On line link resolved to.
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
        match child.map(|child| child.parsed) {
            // `dependency_child_of` never returns `NotALine`, but the
            // type says it can; both mean "no managed line" (R2).
            None | Some(task_dependencies::DependencyLine::NotALine) => {
                self.plan_adoption(view);
            }
            Some(task_dependencies::DependencyLine::Malformed) => {
                self.outcome
                    .uncertain_dependents
                    .insert((view.file_index, view.task_index));
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
                self.plan_empty(view, child_index);
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
                    child_index,
                    links,
                    canonical,
                );
            }
        }
    }
}

impl ReconcileWorker<'_> {
    /// PDS8: skip projecting a `^prj` target in its own note or any
    /// descendant note. Depends-On line links always warn; field-adoption
    /// paths warn only when the target's `[id::]` value is named in the
    /// field. Returns true for every internal target.
    fn warn_project_internal_target(
        &mut self,
        view: &TaskView<'_>,
        path: &Path,
        block: &str,
        target_id: Option<&str>,
        warn_unfielded_line_link: bool,
    ) -> bool {
        if !self
            .project_tree
            .is_project_task(view.file_index, view.task)
            || !self.project_tree.is_internal_target(view.file_index, path)
        {
            return false;
        }
        let same_note = path == view.relative_path;
        let warning_id = target_id
            .map(str::to_string)
            .or_else(|| task_dependencies::dependency_id(path, block).ok())
            .unwrap_or_else(|| block.to_string());
        let field_names_target = view
            .task
            .depends_on
            .iter()
            .any(|field_id| field_id == &warning_id);
        if warn_unfielded_line_link || field_names_target {
            let warning_key = (
                view.relative_path.to_path_buf(),
                view.task.line_index,
                warning_id.clone(),
            );
            if self.warned_project_internal.insert(warning_key) {
                let detail = if warn_unfielded_line_link {
                    if same_note {
                        format!(
                            "Depends-On link to {}#^{} is in the project's own note; kept, not projected",
                            display_path(path),
                            block
                        )
                    } else {
                        format!(
                            "Depends-On link to {}#^{} is inside a sub-project; kept, not projected",
                            display_path(path),
                            block
                        )
                    }
                } else {
                    if same_note {
                        format!(
                            "field id {warning_id} names {}#^{} in the project's own note; kept, not projected",
                            display_path(path),
                            block
                        )
                    } else {
                        format!(
                            "field id {warning_id} names {}#^{} inside a sub-project; kept, not projected",
                            display_path(path),
                            block
                        )
                    }
                };
                self.warn(view, "project_internal_dependency", &detail);
            }
        }
        true
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

impl ReconcileWorker<'_> {
    /// R1 with healing and canonicalisation: resolve every line link,
    /// heal what R3 covers, project the field, and rewrite the line
    /// when it is healed or non-canonical.
    #[allow(clippy::too_many_arguments)]
    fn plan_accepted(
        &mut self,
        view: &TaskView<'_>,
        files: &[FileScan],
        inputs: &mut Vec<InputSnapshot>,
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
            // PDR7: a `^prj` link to a task in its own note is a
            // guaranteed deadlock: kept on the line, never projected
            // into `[dependsOn::]`, and always warned.
            if let LinkTarget::Task {
                path, block, id, ..
            } = &link.resolved
            {
                let path = path.clone();
                let block = block.clone();
                if self.warn_project_internal_target(
                    view,
                    &path,
                    &block,
                    id.as_deref(),
                    true,
                ) {
                    keep_verbatim = true;
                }
            }
            match &link.resolved {
                LinkTarget::Task { id, .. } => {
                    let identity = link
                        .resolved
                        .block_identity()
                        .expect("task target has an identity");
                    if seen.insert(identity.clone()) && !keep_verbatim {
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

    /// R9: a label-only line is the source of truth — it is deleted and
    /// the field goes with it, whatever the field holds.
    fn plan_empty(&mut self, view: &TaskView<'_>, child_index: usize) {
        self.remove_label_only_line(view, child_index);
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

    /// R2: without a line, adopt every field id that has a task with a
    /// block id — write one canonical line for the adoptable ones. Ids
    /// with no `^block-id` anywhere stay in the field with a warning.
    /// Never infer a removal from a missing line.
    fn plan_adoption(&mut self, view: &TaskView<'_>) {
        let uncovered = view.task.depends_on.clone();
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
            if self.warn_project_internal_target(
                view,
                &path,
                &block,
                Some(id),
                false,
            ) {
                continue;
            }
            adopted.push((path, block, id.clone()));
        }
        // Set order mirrors the line the adoption writes.
        let set: Vec<(PathBuf, String, String, bool)> = adopted
            .iter()
            .map(|(path, block, id)| {
                (path.clone(), block.clone(), id.clone(), false)
            })
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
            // R2 keep path: ids staying in the field warn here.
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
        let (position, indent) = self.child_slot(view, None);
        let line =
            task_dependencies::format::format_dependency_line(&indent, &texts);
        self.insert_line(view.file_index, position, &line);
        self.outcome.adopted.push(DependencyLineReport {
            path: display_path(view.relative_path),
            line: position + 1,
            detail: line,
        });
        if next != view.task.depends_on {
            let detail = format!("dependsOn := {}", next.join(", "));
            self.write_dependent_field(view, next.clone(), detail);
        }
    }

    /// Record the dependent's live graph edges for R7 cycle detection.
    /// Only scanned tasks join the graph; archive history, non-tasks,
    /// self links, and unresolved links never do.
    ///
    /// Explicit BLOCKS edges are recorded first, before the block-id
    /// early return, so id-less dependents still appear in
    /// `ReconcileOutcome.explicit_edges`.
    fn record_graph(
        &mut self,
        view: &TaskView<'_>,
        set: &[(PathBuf, String, String, bool)],
    ) {
        let own_identity = view
            .task
            .block_id
            .as_ref()
            .map(|block| (view.relative_path.to_path_buf(), block.clone()));
        let targets: BTreeSet<(PathBuf, String)> = set
            .iter()
            .filter(|(_, _, _, archive)| !archive)
            .map(|(path, block, _, _)| (path.clone(), block.clone()))
            .filter(|target| own_identity.as_ref() != Some(target))
            .collect();
        if !targets.is_empty() {
            self.outcome
                .explicit_edges
                .insert((view.file_index, view.task_index), targets.clone());
        }
        let Some(source) = own_identity else {
            return;
        };
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

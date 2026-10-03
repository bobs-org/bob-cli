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
        for file in files {
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
        }
        let snapshots = files
            .iter()
            .map(|file| {
                let lines = logical_lines(&file.contents);
                let fenced = fenced_lines(&lines, 0..lines.len());
                let mut tasks = file
                    .tasks
                    .iter()
                    .enumerate()
                    .map(|(task_index, _)| task_index)
                    .collect::<Vec<_>>();
                tasks.sort_by_key(|task_index| {
                    file.tasks[*task_index].line_index
                });
                NoteSnapshot {
                    lines: lines.iter().map(|line| line.to_string()).collect(),
                    fenced,
                    line_ending: task_dependencies::format::note_line_ending(
                        &file.contents,
                    ),
                    final_newline: task_dependencies::format::has_final_newline(
                        &file.contents,
                    ),
                    tasks,
                }
            })
            .collect();
        Self {
            vault,
            note_index,
            settings,
            previous_daily_path,
            task_lookup,
            id_lookup,
            non_task_blocks,
            archive_tasks: BTreeMap::new(),
            snapshots,
            graph: BTreeMap::new(),
            graph_nodes: BTreeMap::new(),
            task_rewrites: BTreeMap::new(),
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
                if explicit_archive_reference_path(legacy.target.trim())
                    .is_some()
                {
                    continue;
                }
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
            let text = match &link.resolved {
                LinkTarget::Task { .. } | LinkTarget::Archive { .. } => {
                    let identity = link
                        .resolved
                        .block_identity()
                        .expect("resolved target has an identity");
                    self.shortest_link(view, &identity)
                }
                LinkTarget::NonTask { .. }
                | LinkTarget::SelfTarget
                | LinkTarget::Unresolved => link.verbatim.clone(),
            };
            link.new_text = text.clone();
            line_links.push(text);
            match &link.resolved {
                LinkTarget::Task { id, .. } => {
                    let identity = link
                        .resolved
                        .block_identity()
                        .expect("task target has an identity");
                    if !seen.insert(identity.clone()) {
                        continue;
                    }
                    if let Some(id) =
                        self.project_id(view, files, &identity, id.clone())
                    {
                        set.push((identity.0, identity.1, id, false));
                    }
                }
                LinkTarget::Archive { id, .. } => {
                    let identity = link
                        .resolved
                        .block_identity()
                        .expect("archive target has an identity");
                    if !seen.insert(identity.clone()) {
                        continue;
                    }
                    if let Some(id) = self.archive_id(&identity, id.clone()) {
                        self.outcome.archive_closed_ids.insert(id.clone());
                        set.push((identity.0, identity.1, id, true));
                    }
                }
                LinkTarget::NonTask { .. }
                | LinkTarget::SelfTarget
                | LinkTarget::Unresolved => {}
            }
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
        // Block ids that scanned tasks carry, with their vault-wide ids.
        let mut by_block: BTreeMap<String, Vec<(PathBuf, Option<String>)>> =
            BTreeMap::new();
        for ((path, block), id) in &self.id_lookup {
            by_block
                .entry(block.clone())
                .or_default()
                .push((path.clone(), id.clone()));
        }
        for link in planned.iter_mut() {
            if !matches!(link.resolved, LinkTarget::Unresolved) {
                continue;
            }
            let candidates = by_block
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
    /// stamping the target when it has none (R1 `target_id`).
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
        let canonical =
            task_dependencies::dependency_id(&identity.0, &identity.1).ok()?;
        let (file_index, task_index) =
            self.task_lookup.get(identity).copied()?;
        let target_line_index = files[file_index].tasks[task_index].line_index;
        let current =
            self.pending_or_snapshot_line(file_index, target_line_index);
        let stamped = set_task_fields(
            &current,
            Some(&canonical),
            &files[file_index].tasks[task_index].depends_on,
        );
        if stamped != current {
            self.replace_task_line(
                view,
                file_index,
                target_line_index,
                &stamped,
                Some(canonical.clone()),
                files[file_index].tasks[task_index].depends_on.clone(),
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
        let current = self
            .pending_or_snapshot_line(view.file_index, view.task.line_index);
        let updated =
            set_task_fields(&current, view.task.task_id.as_deref(), &next);
        self.replace_task_line(
            view,
            view.file_index,
            view.task.line_index,
            &updated,
            view.task.task_id.clone(),
            next.clone(),
        );
        let mut detail = format!("dependsOn := {}", next.join(", "));
        if !dropped.is_empty() {
            detail.push_str(&format!("; dropped {}", dropped.join(", ")));
        }
        if !kept.is_empty() {
            detail.push_str(&format!("; heal breadcrumbs {}", kept.join(", ")));
        }
        self.outcome
            .projection_updates
            .push(DependencyProjectionUpdate {
                kind: "dependent_field".to_string(),
                path: display_path(view.relative_path),
                line: view.task.line_index + 1,
                detail,
            });
    }

    /// A label-only line reuses the R2 adoption path: adoptable ids
    /// rewrite it in place, unadoptable ids keep the field with a
    /// warning, and a line with nothing behind it is deleted (R9).
    /// Legacy children are never folded into the line here; the vault
    /// migration owns that write.
    fn plan_empty(
        &mut self,
        view: &TaskView<'_>,
        legacy: &[LegacyMember],
        child_index: usize,
    ) {
        self.plan_adoption(view, legacy, Some(child_index));
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
                // line. The field goes only when no legacy child still
                // needs its coverage.
                self.remove_line(view.file_index, child_index);
                self.outcome.projection_updates.push(
                    DependencyProjectionUpdate {
                        kind: "line_removed".to_string(),
                        path: display_path(view.relative_path),
                        line: child_index + 1,
                        detail: "empty Depends-On line removed".to_string(),
                    },
                );
                if !view.task.depends_on.is_empty() && legacy.is_empty() {
                    let current = self.pending_or_snapshot_line(
                        view.file_index,
                        view.task.line_index,
                    );
                    let updated = set_task_fields(
                        &current,
                        view.task.task_id.as_deref(),
                        &[],
                    );
                    self.replace_task_line(
                        view,
                        view.file_index,
                        view.task.line_index,
                        &updated,
                        view.task.task_id.clone(),
                        Vec::new(),
                    );
                    self.outcome.projection_updates.push(
                        DependencyProjectionUpdate {
                            kind: "dependent_field".to_string(),
                            path: display_path(view.relative_path),
                            line: view.task.line_index + 1,
                            detail: "dependsOn removed".to_string(),
                        },
                    );
                }
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
        for id in &uncovered {
            if own_ids.contains(id) {
                self.warn(
                    view,
                    "self_dependency",
                    &format!(
                        "field id {id} names its own task; kept, not projected"
                    ),
                );
                continue;
            }
            let Some(targets) = by_id.get(id) else {
                self.warn(
                    view,
                    "unadoptable_dependency_id",
                    &format!(
                        "field id {id} matches no task with a block id; kept in the field"
                    ),
                );
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
            // Nothing adoptable: the field keeps everything (R2) and a
            // label-only line is still deleted (R9).
            if let Some(child_index) = empty_line {
                self.remove_line(view.file_index, child_index);
                self.outcome.projection_updates.push(
                    DependencyProjectionUpdate {
                        kind: "line_removed".to_string(),
                        path: display_path(view.relative_path),
                        line: child_index + 1,
                        detail: "empty Depends-On line removed".to_string(),
                    },
                );
            }
            if next != view.task.depends_on {
                let current = self.pending_or_snapshot_line(
                    view.file_index,
                    view.task.line_index,
                );
                let updated = set_task_fields(
                    &current,
                    view.task.task_id.as_deref(),
                    &next,
                );
                self.replace_task_line(
                    view,
                    view.file_index,
                    view.task.line_index,
                    &updated,
                    view.task.task_id.clone(),
                    next.clone(),
                );
            }
            return;
        }
        let texts: Vec<String> = adopted
            .iter()
            .map(|(path, block, _)| {
                self.shortest_link(view, &(path.clone(), block.clone()))
            })
            .collect();
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
            let current = self.pending_or_snapshot_line(
                view.file_index,
                view.task.line_index,
            );
            let updated =
                set_task_fields(&current, view.task.task_id.as_deref(), &next);
            self.replace_task_line(
                view,
                view.file_index,
                view.task.line_index,
                &updated,
                view.task.task_id.clone(),
                next.clone(),
            );
            self.outcome
                .projection_updates
                .push(DependencyProjectionUpdate {
                    kind: "dependent_field".to_string(),
                    path: display_path(view.relative_path),
                    line: view.task.line_index + 1,
                    detail: format!("dependsOn := {}", next.join(", ")),
                });
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

    /// Record a task-line rewrite, merging with any pending stamp
    /// or projection already queued for the same line.
    fn replace_task_line(
        &mut self,
        view: &TaskView<'_>,
        file_index: usize,
        line_index: usize,
        new_line: &str,
        id: Option<String>,
        depends_on: Vec<String>,
    ) {
        let _ = view;
        self.task_rewrites.insert(
            (file_index, line_index),
            (new_line.to_string(), id, depends_on),
        );
        self.outcome.touched_files.insert(file_index);
    }

    /// The current text of a task line: its pending rewrite when one
    /// is queued, else the snapshot.
    fn pending_or_snapshot_line(
        &self,
        file_index: usize,
        line_index: usize,
    ) -> String {
        self.task_rewrites
            .get(&(file_index, line_index))
            .map(|(line, _, _)| line.clone())
            .unwrap_or_else(|| {
                self.snapshots[file_index].lines[line_index].clone()
            })
    }

    fn replace_line(
        &mut self,
        file_index: usize,
        line_index: usize,
        new_line: &str,
    ) {
        self.edits.push(PendingEdit {
            file_index,
            line_index,
            kind: EditKind::Replace,
            new_line: new_line.to_string(),
        });
        self.outcome.touched_files.insert(file_index);
    }

    fn remove_line(&mut self, file_index: usize, line_index: usize) {
        self.edits.push(PendingEdit {
            file_index,
            line_index,
            kind: EditKind::Remove,
            new_line: String::new(),
        });
        self.outcome.touched_files.insert(file_index);
    }

    fn insert_line(
        &mut self,
        file_index: usize,
        line_index: usize,
        new_line: &str,
    ) {
        self.edits.push(PendingEdit {
            file_index,
            line_index,
            kind: EditKind::Insert,
            new_line: new_line.to_string(),
        });
        self.outcome.touched_files.insert(file_index);
    }

    /// Insertion slot for an adopted line: before the first direct
    /// child — or before the second when a `❌ **CANCEL LOG**` child
    /// holds the first slot — with the existing child indent, else
    /// the parent indent plus one tab. `reuse` names a label-only
    /// line whose own indent is reused.
    fn child_slot(
        &self,
        view: &TaskView<'_>,
        reuse: Option<usize>,
    ) -> (usize, String) {
        let snap = &self.snapshots[view.file_index];
        if let Some(child_index) = reuse {
            let indent = leading_bytes(snap.lines[child_index].as_str());
            return (child_index, indent.to_string());
        }
        let refs: Vec<&str> = snap.lines.iter().map(String::as_str).collect();
        let parent_indent = leading_bytes(refs[view.task.line_index]);
        let parent_width =
            leading_indentation_width(refs[view.task.line_index]);
        let mut first_child: Option<(usize, String, bool)> = None;
        for line_index in view.task.line_index + 1..refs.len() {
            let line = refs[line_index];
            if line.trim().is_empty() || snap.fenced.contains(&line_index) {
                continue;
            }
            if leading_indentation_width(line) <= parent_width {
                break;
            }
            if nearest_parent_list_item(&refs, line_index)
                != Some(view.task.line_index)
            {
                continue;
            }
            let cancel = is_cancel_log_line(line);
            if first_child.is_none() {
                first_child =
                    Some((line_index, leading_bytes(line).to_string(), cancel));
                if !cancel {
                    break;
                }
                continue;
            }
            // A second direct child after the Cancel Log: insert here.
            return (line_index, leading_bytes(line).to_string());
        }
        // The shared writer indent: the existing child indent when
        // the task has one, else the parent indent plus one tab.
        let existing =
            first_child.as_ref().map(|(_, indent, _)| indent.as_str());
        let indent = task_dependencies::format::child_indent_for_parent(
            parent_indent,
            existing,
        );
        match first_child {
            Some((line_index, _, true)) => (line_index + 1, indent),
            Some((line_index, _, false)) => (line_index, indent),
            None => (view.task.line_index + 1, indent),
        }
    }

    fn warn(&mut self, view: &TaskView<'_>, kind: &str, detail: &str) {
        self.outcome.warnings.push(DependencyWarning {
            kind: kind.to_string(),
            path: display_path(view.relative_path),
            line: view.task.line_index + 1,
            detail: detail.to_string(),
        });
    }

    /// Materialise every queued edit into note contents. Child-line
    /// edits apply descending so original indices stay valid;
    /// task-line rewrites merge by line. Returns the report.
    fn apply(mut self, files: &mut [FileScan]) -> ReconcileOutcome {
        for ((file_index, line_index), (new_line, _, _)) in &self.task_rewrites
        {
            self.edits.push(PendingEdit {
                file_index: *file_index,
                line_index: *line_index,
                kind: EditKind::Replace,
                new_line: new_line.clone(),
            });
        }
        let mut by_file: BTreeMap<usize, Vec<&PendingEdit>> = BTreeMap::new();
        for edit in &self.edits {
            by_file.entry(edit.file_index).or_default().push(edit);
        }
        for (file_index, mut edits) in by_file {
            edits.sort_by_key(|edit| (edit.line_index, edit.kind as u8));
            edits.reverse();
            let snap = &self.snapshots[file_index];
            let mut lines = snap.lines.clone();
            for edit in edits {
                match edit.kind {
                    EditKind::Replace => {
                        lines[edit.line_index] = edit.new_line.clone();
                    }
                    EditKind::Remove => {
                        lines.remove(edit.line_index);
                    }
                    EditKind::Insert => {
                        lines.insert(edit.line_index, edit.new_line.clone());
                    }
                }
            }
            let joined = lines.join(snap.line_ending);
            files[file_index].contents = if snap.final_newline {
                format!("{joined}{}", snap.line_ending)
            } else {
                joined
            };
        }
        // Deterministic report order: by note, then line.
        self.outcome.projection_updates.sort_by(|left, right| {
            (&left.path, left.line).cmp(&(&right.path, right.line))
        });
        self.outcome.adopted.sort_by(|left, right| {
            (&left.path, left.line).cmp(&(&right.path, right.line))
        });
        self.outcome.healed.sort_by(|left, right| {
            (&left.path, left.line).cmp(&(&right.path, right.line))
        });
        self.outcome.canonicalized.sort_by(|left, right| {
            (&left.path, left.line).cmp(&(&right.path, right.line))
        });
        self.outcome.warnings.sort_by(|left, right| {
            (&left.kind, &left.path, left.line).cmp(&(
                &right.kind,
                &right.path,
                right.line,
            ))
        });
        self.outcome
    }

    fn warn_cycle(
        &mut self,
        files: &[FileScan],
        start: &(PathBuf, String),
        stack: &[(PathBuf, String)],
    ) {
        // Rotate the discovered walk so the path starts at the
        // smallest member; the walk already ends back at `start`.
        let mut ordered = stack.to_vec();
        ordered.push(start.clone());
        let first = ordered.iter().min().cloned().unwrap_or(start.clone());
        let position =
            ordered.iter().position(|node| *node == first).unwrap_or(0);
        let mut rotated = ordered[position..].to_vec();
        rotated.extend_from_slice(&ordered[1..position + 1]);
        let path = rotated
            .iter()
            .map(|(path, block)| format!("{}#^{}", display_path(path), block))
            .collect::<Vec<_>>()
            .join(" -> ");
        let (file_index, line_index) =
            self.graph_nodes.get(&first).copied().unwrap_or((0, 0));
        let relative = files
            .get(file_index)
            .map(|file| display_path(&file.relative_path))
            .unwrap_or_default();
        self.outcome.warnings.push(DependencyWarning {
            kind: "dependency_cycle".to_string(),
            path: relative,
            line: line_index + 1,
            detail: format!("dependency cycle: {path}"),
        });
    }
}

/// Rewrite a task line's `[id::]` / `[dependsOn::]` suffix: existing
/// trailing Dataview fields are peeled, `id` and `dependsOn` are
/// dropped, and the survivors are re-emitted with the new values in
/// Tasks key order (`id` before `dependsOn`), right of any `fresh`
/// and before the trailing `^block-id`.
fn set_task_fields(
    line: &str,
    task_id: Option<&str>,
    depends_on: &[String],
) -> String {
    let trimmed = line.trim_end();
    let known_block = trailing_block_id(trimmed);
    let (mut stem, block) = match known_block {
        Some(block) => match trimmed
            .strip_suffix(&format!("^{block}"))
            .map(str::trim_end)
        {
            Some(stem) => (stem, Some(block)),
            None => (trimmed, None),
        },
        None => (trimmed, None),
    };
    let mut kept = Vec::new();
    while let Some((start, key, _)) = trailing_dataview_field(stem) {
        let raw = stem[start..].trim_end().to_string();
        stem = stem[..start].trim_end();
        if key == "id" || key == "dependsOn" {
            continue;
        }
        kept.push(raw);
    }
    kept.reverse();
    let mut rebuilt = stem.to_string();
    for field in kept {
        rebuilt.push(' ');
        rebuilt.push_str(&field);
    }
    if let Some(id) = task_id {
        rebuilt.push_str(&format!(" [id:: {id}]"));
    }
    if !depends_on.is_empty() {
        rebuilt.push_str(&format!(" [dependsOn:: {}]", depends_on.join(", ")));
    }
    if let Some(block) = block {
        rebuilt.push_str(&format!(" ^{block}"));
    }
    rebuilt
}

/// Rebuild a Depends-On child line around new link texts, preserving
/// the original indent and list marker.
fn rebuild_child_line(original: &str, link_texts: &[String]) -> String {
    let indent_len = original
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    let marker_end =
        after_list_marker(original, indent_len).unwrap_or(indent_len);
    format!(
        "{} {} **DEPENDS ON:**{}",
        &original[..marker_end],
        task_dependencies::format::DEPENDS_ON_EMOJI,
        if link_texts.is_empty() {
            String::new()
        } else {
            format!(
                " {}",
                link_texts
                    .join(task_dependencies::format::DEPENDS_ON_SEPARATOR)
            )
        }
    )
}

/// Leading whitespace bytes of a line (the indent prefix to reuse).
fn leading_bytes(line: &str) -> &str {
    let len = line
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    &line[..len]
}

/// Whether a child bullet is the `❌ **CANCEL LOG**` slot: an adopted
/// line goes second when it holds the first slot (`contract` §2.1).
fn is_cancel_log_line(line: &str) -> bool {
    let indent_len = line
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    let rest = match after_list_marker(line, indent_len) {
        Some(marker_end) => line[marker_end..].trim_start(),
        None => return false,
    };
    rest.starts_with('❌') && rest.contains("CANCEL LOG")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn depends(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn field_writer_places_id_before_depends_on_before_block_id() {
        assert_eq!(
            set_task_fields(
                "- [ ] #task Ship it ^ship",
                Some("tasks__ship"),
                &depends(&["tasks__tests"]),
            ),
            "- [ ] #task Ship it [id:: tasks__ship] [dependsOn:: tasks__tests] ^ship"
        );
    }

    #[test]
    fn field_writer_lands_right_of_fresh_and_replaces_stale() {
        assert_eq!(
            set_task_fields(
                "- [ ] #task Ship it [fresh:: 2026-10-02] [dependsOn:: stale] [scheduled:: 2026-10-03] ^ship",
                Some("tasks__ship"),
                &depends(&["tasks__tests"]),
            ),
            "- [ ] #task Ship it [fresh:: 2026-10-02] [scheduled:: 2026-10-03] [id:: tasks__ship] [dependsOn:: tasks__tests] ^ship"
        );
    }

    #[test]
    fn field_writer_removes_depends_on_when_empty() {
        assert_eq!(
            set_task_fields(
                "- [ ] #task Ship it [id:: tasks__ship] [dependsOn:: tasks__tests] ^ship",
                Some("tasks__ship"),
                &[],
            ),
            "- [ ] #task Ship it [id:: tasks__ship] ^ship"
        );
    }

    #[test]
    fn field_writer_drops_both_fields_for_r9() {
        assert_eq!(
            set_task_fields(
                "- [ ] #task Bare [id:: tasks__bare] [dependsOn:: tasks__ghost]",
                None,
                &[],
            ),
            "- [ ] #task Bare"
        );
    }

    #[test]
    fn field_writer_replaces_parenthesized_metadata() {
        assert_eq!(
            set_task_fields(
                "- [ ] #task Ship it (dependsOn:: tasks__tests) ^ship",
                None,
                &depends(&["tasks__other"]),
            ),
            "- [ ] #task Ship it [dependsOn:: tasks__other] ^ship"
        );
    }
}

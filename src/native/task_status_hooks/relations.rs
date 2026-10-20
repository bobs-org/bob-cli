//! LINKED FROM projection pass (`docs/task-relations.md` §§5–7).
//!
//! Runs immediately after `reconcile_blocks_lines`. It never rewrites an
//! authored LINKS line: desired membership comes from accepted relation
//! edges, plus existing LINKED FROM entries that name uncertain sources
//! so those are never pruned.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use super::super::capture_block_ids::mint_block_id;
use super::super::collect_done::block_ids_in_markdown;
use super::super::task_dependencies::blocks::{BlocksEdit, BlocksLinePlan};
use super::super::task_dependencies::format::{
    canonical_link, has_final_newline, note_line_ending,
};
use super::super::task_dependencies::is_archive_path;
use super::super::task_relations::parse::{
    linked_from_children_of, links_children_of,
};
use super::super::task_relations::plan::plan_linked_from_line;
use super::super::task_relations::{LinkedFromLine, LinksLine, RelationEntry};
use super::super::vault_links::{target_to_markdown_path, NoteIndex};
use super::*;

/// Outcome of one LINKED FROM projection pass: content edits already
/// applied to `files`, plus the additive report rows.
pub(super) struct RelationsOutcome {
    pub(super) touched_files: BTreeSet<usize>,
    pub(super) original_contents: BTreeMap<usize, String>,
    pub(super) updates: Vec<BlocksLineUpdate>,
    pub(super) mints: Vec<MintedDependentBlockId>,
    pub(super) warnings: Vec<RelationWarning>,
}

/// Load `done/**/*.md` notes that carry relation sources or explicit
/// `done/` targets, read-only. Byte-prefilter for `LINKS`, then load any
/// remaining explicit archive targets named from active or archive LINKS
/// lines so those identities resolve.
pub(super) fn load_relation_archives(
    vault: &Path,
    settings: &TasksSettings,
    files: &[FileScan],
    inputs: &mut Vec<InputSnapshot>,
) -> Result<Vec<FileScan>, SyncError> {
    let done_root = vault.join("done");
    let mut archives = Vec::new();
    let mut loaded = BTreeSet::new();
    if done_root.is_dir() {
        let mut paths = Vec::new();
        collect_markdown_files(&done_root, &mut paths).map_err(|error| {
            SyncError::io("scan archive notes", &done_root, error)
        })?;
        for path in paths {
            let Some(scan) = load_archive_file(vault, settings, &path, inputs)?
            else {
                continue;
            };
            if scan.contents.contains("LINKS") {
                loaded.insert(scan.relative_path.clone());
                archives.push(scan);
            }
        }
    }

    let mut extra_targets = BTreeSet::new();
    collect_explicit_archive_targets(files, &mut extra_targets);
    collect_explicit_archive_targets(&archives, &mut extra_targets);
    for relative in extra_targets {
        if !loaded.insert(relative.clone()) {
            continue;
        }
        let path = vault.join(&relative);
        if let Some(scan) = load_archive_file(vault, settings, &path, inputs)? {
            archives.push(scan);
        }
    }
    archives
        .sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(archives)
}

fn load_archive_file(
    vault: &Path,
    settings: &TasksSettings,
    path: &Path,
    inputs: &mut Vec<InputSnapshot>,
) -> Result<Option<FileScan>, SyncError> {
    if !path.is_file() {
        return Ok(None);
    }
    let snapshot = match capture_optional(path, InputKind::Archive) {
        Ok(snapshot) => snapshot,
        Err(CaptureError::Unsupported { .. }) => return Ok(None),
        Err(error) => {
            return Err(capture_to_sync("read archive note", error));
        }
    };
    if snapshot.is_missing() {
        push_unique_input(inputs, snapshot);
        return Ok(None);
    }
    let contents = match snapshot.utf8_contents() {
        Ok(Some(contents)) => contents,
        Ok(None) => {
            push_unique_input(inputs, snapshot);
            return Ok(None);
        }
        Err(error) => {
            return Err(capture_to_sync("read archive note", error));
        }
    };
    let relative_path = path
        .strip_prefix(vault)
        .map_or_else(|_| path.to_path_buf(), Path::to_path_buf);
    push_unique_input(inputs, snapshot);
    let tasks = parse_tasks(&contents, settings);
    Ok(Some(FileScan {
        path: path.to_path_buf(),
        relative_path,
        contents,
        tasks,
    }))
}

fn collect_explicit_archive_targets(
    files: &[FileScan],
    targets: &mut BTreeSet<PathBuf>,
) {
    for file in files {
        if !file.contents.contains("LINKS") {
            continue;
        }
        let lines = logical_lines(&file.contents);
        let line_refs: Vec<&str> = lines.iter().copied().collect();
        let fenced = fenced_lines(&line_refs, 0..line_refs.len());
        for task in &file.tasks {
            for child in links_children_of(&line_refs, &fenced, task.line_index)
            {
                let LinksLine::Accepted { entries, .. } = &child.parsed else {
                    continue;
                };
                for entry in entries {
                    if let Some(path) =
                        explicit_archive_reference_path(&entry.target)
                    {
                        targets.insert(path);
                    }
                }
            }
        }
    }
}

/// Project LINKED FROM lines from accepted relation edges.
///
/// `files` is mutated in place (note contents only). Callers must
/// re-parse touched files afterwards. `previous_daily_file` and every
/// archive note are never written. Sources whose mint would land in
/// `quiet_deferred` are skipped this run (LR5).
#[allow(clippy::too_many_arguments)]
pub(super) fn reconcile_relation_lines(
    files: &mut [FileScan],
    note_index: &NoteIndex,
    archives: &[FileScan],
    settings: &TasksSettings,
    previous_daily_file: Option<usize>,
    quiet_deferred: &BTreeSet<usize>,
    enabled: bool,
) -> RelationsOutcome {
    let mut pass = RelationsPass {
        files,
        note_index,
        archives,
        settings,
        previous_daily_file,
        quiet_deferred,
        enabled,
        outcome: RelationsOutcome {
            touched_files: BTreeSet::new(),
            original_contents: BTreeMap::new(),
            updates: Vec::new(),
            mints: Vec::new(),
            warnings: Vec::new(),
        },
        used: BTreeMap::new(),
    };
    pass.run();
    pass.outcome
}

struct RelationsPass<'a> {
    files: &'a mut [FileScan],
    note_index: &'a NoteIndex,
    archives: &'a [FileScan],
    settings: &'a TasksSettings,
    previous_daily_file: Option<usize>,
    quiet_deferred: &'a BTreeSet<usize>,
    enabled: bool,
    outcome: RelationsOutcome,
    used: BTreeMap<usize, HashSet<String>>,
}

#[derive(Clone, Copy)]
enum TaskRef {
    Active { file: usize, task: usize },
    Archive { file: usize, task: usize },
}

struct CollectedEdges {
    /// Certain (S, T, label) edges keyed by target identity.
    by_target: BTreeMap<(PathBuf, String), Vec<EdgeSource>>,
    uncertain: BTreeSet<(PathBuf, String)>,
    mint_needed: BTreeSet<(usize, usize)>,
}

struct EdgeSource {
    path: PathBuf,
    loc: TaskRef,
    label: Option<String>,
}

impl RelationsPass<'_> {
    fn run(&mut self) {
        let collected = self.collect_edges();
        if self.enabled {
            self.mint_sources(&collected.mint_needed);
        }
        let owners = self.collect_owners(&collected);
        let mut planned: BTreeMap<usize, Vec<BlocksEdit>> = BTreeMap::new();
        for owner in &owners {
            if let Some((file_index, edits)) =
                self.plan_owner(*owner, &collected)
            {
                planned.entry(file_index).or_default().extend(edits);
            }
        }
        for (file_index, edits) in planned {
            self.apply_edits(file_index, edits);
        }
        self.outcome.updates.sort_by(|left, right| {
            (&left.path, left.line).cmp(&(&right.path, right.line))
        });
        self.outcome.mints.sort_by(|left, right| {
            (&left.path, left.line).cmp(&(&right.path, right.line))
        });
        self.outcome.warnings.sort_by(|left, right| {
            (&left.kind, &left.path, left.line).cmp(&(
                &right.kind,
                &right.path,
                right.line,
            ))
        });
        for file_index in &self.outcome.touched_files {
            let file = &mut self.files[*file_index];
            file.tasks = parse_tasks(&file.contents, self.settings);
        }
    }

    fn writable(&self, file_index: usize) -> bool {
        self.previous_daily_file != Some(file_index)
    }

    fn collect_edges(&mut self) -> CollectedEdges {
        let mut collected = CollectedEdges {
            by_target: BTreeMap::new(),
            uncertain: BTreeSet::new(),
            mint_needed: BTreeSet::new(),
        };
        let active_count = self.files.len();
        for file_index in 0..active_count {
            self.collect_file_edges(file_index, false, &mut collected);
        }
        let archive_count = self.archives.len();
        for file_index in 0..archive_count {
            self.collect_file_edges(file_index, true, &mut collected);
        }
        collected
    }

    fn collect_file_edges(
        &mut self,
        file_index: usize,
        archive: bool,
        collected: &mut CollectedEdges,
    ) {
        let contents_has_links = if archive {
            self.archives[file_index].contents.contains("LINKS")
        } else {
            self.files[file_index].contents.contains("LINKS")
        };
        if !contents_has_links {
            return;
        }
        let relative = if archive {
            self.archives[file_index].relative_path.clone()
        } else {
            self.files[file_index].relative_path.clone()
        };
        let contents = if archive {
            self.archives[file_index].contents.clone()
        } else {
            self.files[file_index].contents.clone()
        };
        let tasks = if archive {
            self.archives[file_index].tasks.clone()
        } else {
            self.files[file_index].tasks.clone()
        };
        let duplicates = duplicate_block_ids(&tasks);
        let lines = logical_lines(&contents);
        let line_refs: Vec<&str> = lines.iter().copied().collect();
        let fenced = fenced_lines(&line_refs, 0..line_refs.len());
        for (task_index, task) in tasks.iter().enumerate() {
            let children =
                links_children_of(&line_refs, &fenced, task.line_index);
            if children.is_empty() {
                continue;
            }
            let loc = if archive {
                TaskRef::Archive {
                    file: file_index,
                    task: task_index,
                }
            } else {
                TaskRef::Active {
                    file: file_index,
                    task: task_index,
                }
            };
            let malformed = children
                .iter()
                .any(|child| matches!(child.parsed, LinksLine::Malformed));
            if malformed {
                let line = children
                    .iter()
                    .find(|child| matches!(child.parsed, LinksLine::Malformed))
                    .map(|child| child.line_index + 1)
                    .unwrap_or(task.line_index + 1);
                self.outcome.warnings.push(RelationWarning {
                    kind: "malformed_links_line".to_string(),
                    path: display_path(&relative),
                    line,
                    message: "LINKS line has trailing prose, a half-typed link, or other residue; left alone".to_string(),
                });
            }
            let duplicated = task
                .block_id
                .as_ref()
                .is_some_and(|id| duplicates.contains(id));
            if duplicated {
                if let Some(block_id) = &task.block_id {
                    self.outcome.warnings.push(RelationWarning {
                        kind: "duplicate_block_id".to_string(),
                        path: display_path(&relative),
                        line: task.line_index + 1,
                        message: format!(
                            "note has more than one ^{block_id}; source not listed this run"
                        ),
                    });
                    collected
                        .uncertain
                        .insert((relative.clone(), block_id.clone()));
                }
            }
            if malformed {
                if let Some(block_id) = &task.block_id {
                    collected
                        .uncertain
                        .insert((relative.clone(), block_id.clone()));
                }
                continue;
            }
            if duplicated {
                continue;
            }
            for child in &children {
                let LinksLine::Accepted { entries, .. } = &child.parsed else {
                    continue;
                };
                for entry in entries {
                    self.record_edge(
                        &relative,
                        task,
                        loc,
                        archive,
                        file_index,
                        child.line_index + 1,
                        entry,
                        collected,
                    );
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn record_edge(
        &mut self,
        source_path: &Path,
        source: &TaskLine,
        loc: TaskRef,
        archive: bool,
        file_index: usize,
        line: usize,
        entry: &RelationEntry,
        collected: &mut CollectedEdges,
    ) {
        match self.resolve_entry(source_path, source, entry) {
            Err(kind) => {
                self.outcome.warnings.push(RelationWarning {
                    kind: kind.to_string(),
                    path: display_path(source_path),
                    line,
                    message: warning_message(kind, entry),
                });
            }
            Ok(target) => {
                if source.block_id.is_none() {
                    let mintable = !archive
                        && self.writable(file_index)
                        && !self.quiet_deferred.contains(&file_index);
                    if mintable {
                        collected
                            .mint_needed
                            .insert((file_index, task_index_of(loc)));
                    } else {
                        return;
                    }
                }
                collected.by_target.entry(target).or_default().push(
                    EdgeSource {
                        path: source_path.to_path_buf(),
                        loc,
                        label: entry.label.clone(),
                    },
                );
            }
        }
    }

    fn resolve_entry(
        &self,
        source_path: &Path,
        source: &TaskLine,
        entry: &RelationEntry,
    ) -> Result<(PathBuf, String), &'static str> {
        let Some(path) = self.resolve_target_path(source_path, &entry.target)
        else {
            return Err("unresolved_relation_link");
        };
        if source.block_id.as_deref() == Some(entry.block_id.as_str())
            && path == source_path
        {
            return Err("self_relation");
        }
        if self.lookup_task(&path, &entry.block_id).is_some() {
            return Ok((path, entry.block_id.clone()));
        }
        if self.id_exists(&path, &entry.block_id) {
            return Err("relation_target_not_task");
        }
        Err("unresolved_relation_link")
    }

    fn resolve_target_path(
        &self,
        source_path: &Path,
        target: &str,
    ) -> Option<PathBuf> {
        if target.is_empty() {
            return Some(source_path.to_path_buf());
        }
        let candidate = target_to_markdown_path(target)?;
        if is_archive_path(&candidate) {
            if self
                .archives
                .iter()
                .any(|file| file.relative_path == candidate)
            {
                return Some(candidate);
            }
            return None;
        }
        self.note_index.resolve(Some(source_path), target)
    }

    fn lookup_task(&self, path: &Path, block_id: &str) -> Option<TaskRef> {
        for (file_index, file) in self.files.iter().enumerate() {
            if file.relative_path != path {
                continue;
            }
            for (task_index, task) in file.tasks.iter().enumerate() {
                if task.block_id.as_deref() == Some(block_id) {
                    return Some(TaskRef::Active {
                        file: file_index,
                        task: task_index,
                    });
                }
            }
        }
        for (file_index, file) in self.archives.iter().enumerate() {
            if file.relative_path != path {
                continue;
            }
            for (task_index, task) in file.tasks.iter().enumerate() {
                if task.block_id.as_deref() == Some(block_id) {
                    return Some(TaskRef::Archive {
                        file: file_index,
                        task: task_index,
                    });
                }
            }
        }
        None
    }

    fn id_exists(&self, path: &Path, block_id: &str) -> bool {
        for file in self.files.iter().chain(self.archives.iter()) {
            if file.relative_path != path {
                continue;
            }
            return block_ids_in_markdown(&file.contents)
                .iter()
                .any(|id| id == block_id);
        }
        false
    }

    fn mint_sources(&mut self, needed: &BTreeSet<(usize, usize)>) {
        for (file_index, task_index) in needed {
            if self.files[*file_index].tasks[*task_index]
                .block_id
                .is_some()
            {
                continue;
            }
            self.mint_one(*file_index, *task_index);
        }
    }

    fn mint_one(&mut self, file_index: usize, task_index: usize) {
        let relative = self.files[file_index].relative_path.clone();
        let line_index = self.files[file_index].tasks[task_index].line_index;
        let description =
            self.files[file_index].tasks[task_index].description.clone();
        let used = self.used_for(file_index);
        let used_refs: HashSet<&str> =
            used.iter().map(String::as_str).collect();
        let minted = mint_block_id(&title_for_mint(&description), &used_refs);
        let lines = logical_lines(&self.files[file_index].contents);
        let Some(current) = lines.get(line_index) else {
            return;
        };
        let updated = append_trailing_block_id(current, &minted);
        self.touch(file_index);
        self.files[file_index].contents = replace_logical_line_text(
            &self.files[file_index].contents,
            line_index,
            &updated,
        );
        self.files[file_index].tasks[task_index].block_id =
            Some(minted.clone());
        self.used
            .entry(file_index)
            .or_default()
            .insert(minted.clone());
        self.outcome.mints.push(MintedDependentBlockId {
            path: display_path(&relative),
            line: line_index + 1,
            block_id: minted,
        });
    }

    fn used_for(&mut self, file_index: usize) -> HashSet<String> {
        if let Some(used) = self.used.get(&file_index) {
            return used.clone();
        }
        let used = block_ids_in_markdown(&self.files[file_index].contents)
            .into_iter()
            .collect::<HashSet<_>>();
        self.used.insert(file_index, used.clone());
        used
    }

    fn collect_owners(
        &self,
        collected: &CollectedEdges,
    ) -> BTreeSet<(usize, usize)> {
        let mut owners = BTreeSet::new();
        if self.enabled {
            for (target, _) in &collected.by_target {
                if let Some(TaskRef::Active { file, task }) =
                    self.lookup_task(&target.0, &target.1)
                    && self.writable(file)
                {
                    owners.insert((file, task));
                }
            }
        }
        for (file_index, file) in self.files.iter().enumerate() {
            if !self.writable(file_index) {
                continue;
            }
            if !file.contents.contains("LINKED FROM")
                && !file.contents.contains("BACKLINKS")
            {
                continue;
            }
            let lines = logical_lines(&file.contents);
            let line_refs: Vec<&str> = lines.iter().copied().collect();
            let fenced = fenced_lines(&line_refs, 0..line_refs.len());
            for (task_index, task) in file.tasks.iter().enumerate() {
                if !linked_from_children_of(
                    &line_refs,
                    &fenced,
                    task.line_index,
                )
                .is_empty()
                {
                    owners.insert((file_index, task_index));
                }
            }
        }
        owners
    }

    fn plan_owner(
        &mut self,
        owner: (usize, usize),
        collected: &CollectedEdges,
    ) -> Option<(usize, Vec<BlocksEdit>)> {
        let (file_index, task_index) = owner;
        let relative = self.files[file_index].relative_path.clone();
        let task = self.files[file_index].tasks[task_index].clone();
        if !self.writable(file_index) {
            return None;
        }
        let lines_owned: Vec<String> =
            logical_lines(&self.files[file_index].contents)
                .into_iter()
                .map(str::to_string)
                .collect();
        let line_refs: Vec<&str> =
            lines_owned.iter().map(String::as_str).collect();
        let fenced = fenced_lines(&line_refs, 0..line_refs.len());
        let children =
            linked_from_children_of(&line_refs, &fenced, task.line_index);
        if children
            .iter()
            .any(|child| matches!(child.parsed, LinkedFromLine::Malformed))
        {
            let line = children
                .iter()
                .find(|child| matches!(child.parsed, LinkedFromLine::Malformed))
                .map(|child| child.line_index + 1)
                .unwrap_or(task.line_index + 1);
            self.outcome.warnings.push(RelationWarning {
                kind: "malformed_linked_from_line".to_string(),
                path: display_path(&relative),
                line,
                message: "LINKED FROM line has trailing prose, a half-typed link, or other residue; left alone".to_string(),
            });
            return None;
        }
        let desired =
            self.desired_entries(&relative, &task, &children, collected);
        match plan_linked_from_line(
            &line_refs,
            &fenced,
            task.line_index,
            &desired,
        ) {
            BlocksLinePlan::Noop => None,
            BlocksLinePlan::Malformed => {
                self.outcome.warnings.push(RelationWarning {
                    kind: "malformed_linked_from_line".to_string(),
                    path: display_path(&relative),
                    line: task.line_index + 1,
                    message: "LINKED FROM line has trailing prose, a half-typed link, or other residue; left alone".to_string(),
                });
                None
            }
            BlocksLinePlan::Edits { edits, report } => {
                let line = edits
                    .first()
                    .map(edit_line_index)
                    .unwrap_or(task.line_index)
                    + 1;
                self.outcome.updates.push(BlocksLineUpdate {
                    path: display_path(&relative),
                    line,
                    task: task.description.clone(),
                    kind: report.kind.as_str().to_string(),
                    added: report.added,
                    removed: report.removed,
                });
                Some((file_index, edits))
            }
        }
    }

    fn desired_entries(
        &self,
        owner_path: &Path,
        owner: &TaskLine,
        children: &[super::super::task_relations::parse::LinkedFromChild],
        collected: &CollectedEdges,
    ) -> BTreeSet<(String, Option<String>)> {
        if !self.enabled {
            return BTreeSet::new();
        }
        let Some(owner_id) = &owner.block_id else {
            return BTreeSet::new();
        };
        let identity = (owner_path.to_path_buf(), owner_id.clone());
        let mut desired = BTreeMap::new();
        if let Some(sources) = collected.by_target.get(&identity) {
            for source in sources {
                let Some(block_id) = self.source_block_id(source.loc) else {
                    continue;
                };
                if collected
                    .uncertain
                    .contains(&(source.path.clone(), block_id.clone()))
                {
                    continue;
                }
                let link = canonical_link(
                    &source.path,
                    &block_id,
                    owner_path,
                    self.note_index,
                );
                desired
                    .entry((source.path.clone(), block_id))
                    .or_insert((link, source.label.clone()));
            }
        }
        for child in children {
            let LinkedFromLine::Accepted { entries, .. } = &child.parsed else {
                continue;
            };
            for entry in entries {
                let Some(path) =
                    self.resolve_target_path(owner_path, &entry.target)
                else {
                    continue;
                };
                let key = (path.clone(), entry.block_id.clone());
                if collected.uncertain.contains(&key) {
                    let link = canonical_link(
                        &path,
                        &entry.block_id,
                        owner_path,
                        self.note_index,
                    );
                    desired.entry(key).or_insert((link, entry.label.clone()));
                }
            }
        }
        desired.into_values().collect()
    }

    fn source_block_id(&self, loc: TaskRef) -> Option<String> {
        match loc {
            TaskRef::Active { file, task } => {
                self.files.get(file)?.tasks.get(task)?.block_id.clone()
            }
            TaskRef::Archive { file, task } => {
                self.archives.get(file)?.tasks.get(task)?.block_id.clone()
            }
        }
    }

    fn apply_edits(&mut self, file_index: usize, edits: Vec<BlocksEdit>) {
        if edits.is_empty() {
            return;
        }
        self.touch(file_index);
        let ending = note_line_ending(&self.files[file_index].contents);
        let final_newline = has_final_newline(&self.files[file_index].contents);
        let mut lines: Vec<String> =
            logical_lines(&self.files[file_index].contents)
                .into_iter()
                .map(str::to_string)
                .collect();
        let mut ordered = edits;
        ordered.sort_by(|left, right| {
            edit_line_index(right)
                .cmp(&edit_line_index(left))
                .then(edit_rank(left).cmp(&edit_rank(right)))
        });
        for edit in ordered {
            match edit {
                BlocksEdit::Replace { line_index, text } => {
                    if line_index < lines.len() {
                        lines[line_index] = text;
                    }
                }
                BlocksEdit::Remove { line_index } => {
                    if line_index < lines.len() {
                        lines.remove(line_index);
                    }
                }
                BlocksEdit::Insert { line_index, text } => {
                    let index = line_index.min(lines.len());
                    lines.insert(index, text);
                }
            }
        }
        let joined = lines.join(ending);
        self.files[file_index].contents = if final_newline {
            format!("{joined}{ending}")
        } else {
            joined
        };
    }

    fn touch(&mut self, file_index: usize) {
        if self.outcome.touched_files.insert(file_index) {
            self.outcome
                .original_contents
                .insert(file_index, self.files[file_index].contents.clone());
        }
    }
}

fn duplicate_block_ids(tasks: &[TaskLine]) -> BTreeSet<String> {
    let mut counts = BTreeMap::new();
    for task in tasks {
        if let Some(block_id) = &task.block_id {
            *counts.entry(block_id.clone()).or_insert(0usize) += 1;
        }
    }
    counts
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(id, _)| id)
        .collect()
}

fn task_index_of(loc: TaskRef) -> usize {
    match loc {
        TaskRef::Active { task, .. } | TaskRef::Archive { task, .. } => task,
    }
}

fn warning_message(kind: &str, entry: &RelationEntry) -> String {
    let link = entry.canonical_text();
    match kind {
        "self_relation" => {
            format!("{link} names the source task; omitted from the mirror")
        }
        "relation_target_not_task" => {
            format!(
                "{link} does not resolve to a #task; omitted from the mirror"
            )
        }
        _ => format!(
            "{link} does not resolve to a task; omitted from the mirror"
        ),
    }
}

fn title_for_mint(description: &str) -> String {
    let mut state = description;
    for _ in 0..20 {
        if let Some((start, _, _)) = trailing_dataview_field(state) {
            state = state[..start].trim_end();
            continue;
        }
        if let Some(start) = trailing_task_tag_start(state) {
            state = state[..start].trim_end();
            continue;
        }
        break;
    }
    state.to_string()
}

fn append_trailing_block_id(line: &str, block_id: &str) -> String {
    format!("{} ^{block_id}", line.trim_end())
}

fn replace_logical_line_text(
    contents: &str,
    line_index: usize,
    new_line: &str,
) -> String {
    let mut output = String::with_capacity(contents.len() + new_line.len());
    for (index, segment) in contents.split_inclusive('\n').enumerate() {
        if index == line_index {
            let ending = if segment.ends_with("\r\n") {
                "\r\n"
            } else if segment.ends_with('\n') {
                "\n"
            } else {
                ""
            };
            output.push_str(new_line);
            output.push_str(ending);
        } else {
            output.push_str(segment);
        }
    }
    output
}

fn edit_line_index(edit: &BlocksEdit) -> usize {
    match edit {
        BlocksEdit::Insert { line_index, .. }
        | BlocksEdit::Replace { line_index, .. }
        | BlocksEdit::Remove { line_index } => *line_index,
    }
}

fn edit_rank(edit: &BlocksEdit) -> u8 {
    match edit {
        BlocksEdit::Replace { .. } => 0,
        BlocksEdit::Remove { .. } => 1,
        BlocksEdit::Insert { .. } => 2,
    }
}

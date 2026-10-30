//! LINKED FROM projection pass (`docs/task-relations.md` §§5–7, §19).
//!
//! Runs immediately after `reconcile_blocks_lines`. It never rewrites an
//! authored LINKS line: desired membership comes from accepted relation
//! edges, mention pairs that pass M7, plus existing LINKED FROM entries
//! that name uncertain sources so those are never pruned.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use super::super::capture_block_ids::mint_block_id;
use super::super::collect_done::block_ids_in_markdown;
use super::super::task_dependencies::blocks::{BlocksEdit, BlocksLinePlan};
use super::super::task_dependencies::format::{
    canonical_link, has_final_newline, note_line_ending,
};
use super::super::task_dependencies::is_archive_path;
use super::super::task_dependencies::parse::{
    dependency_child_of, DependencyLine,
};
use super::super::task_relations::mentions::{
    has_unclosed_fence, scan_mentions, MentionPair,
};
use super::super::task_relations::parse::{
    linked_from_children_of, links_children_of,
};
use super::super::task_relations::plan::plan_linked_from_line;
use super::super::task_relations::{
    LinkedFromLine, LinksLine, RelationEntry, RelationEntryKind, MENTION_LABEL,
};
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
    pub(super) mention_edges: usize,
}

/// Load `done/**/*.md` notes that carry relation sources or explicit
/// `done/` targets, read-only. Byte-prefilter for `LINKS`, and when
/// mentions are enabled also for notes that contain both `#task` and
/// `#^`. Then load any remaining explicit archive targets named from
/// active or archive LINKS lines so those identities resolve.
pub(super) fn load_relation_archives(
    vault: &Path,
    settings: &TasksSettings,
    files: &[FileScan],
    inputs: &mut Vec<InputSnapshot>,
    mentions_enabled: bool,
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
            if admits_relation_scan(&scan.contents, mentions_enabled) {
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
        let line_refs: Vec<&str> = lines.to_vec();
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
    mentions_enabled: bool,
) -> RelationsOutcome {
    let mut pass = RelationsPass {
        files,
        note_index,
        archives,
        settings,
        previous_daily_file,
        quiet_deferred,
        enabled,
        mentions_enabled,
        outcome: RelationsOutcome {
            touched_files: BTreeSet::new(),
            original_contents: BTreeMap::new(),
            updates: Vec::new(),
            mints: Vec::new(),
            warnings: Vec::new(),
            mention_edges: 0,
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
    mentions_enabled: bool,
    outcome: RelationsOutcome,
    used: BTreeMap<usize, HashSet<String>>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TaskRef {
    Active { file: usize, task: usize },
    Archive { file: usize, task: usize },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MintReason {
    Relation,
    Mention,
}

impl MintReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Relation => "relation",
            Self::Mention => "mention",
        }
    }
}

type TaskIdentity = (PathBuf, String);

struct CollectedEdges {
    /// Certain (S, T, label) edges keyed by target identity.
    by_target: BTreeMap<TaskIdentity, Vec<EdgeSource>>,
    mention_by_target: BTreeMap<TaskIdentity, Vec<MentionSource>>,
    uncertain: BTreeSet<TaskIdentity>,
    /// Mention-only uncertainty (unclosed fence, malformed Depends-On on S).
    mention_uncertain: BTreeSet<TaskIdentity>,
    malformed_depends: BTreeSet<TaskIdentity>,
    mint_needed: BTreeMap<(usize, usize), MintReason>,
}

impl CollectedEdges {
    fn note_mint(&mut self, file: usize, task: usize, reason: MintReason) {
        self.mint_needed
            .entry((file, task))
            .and_modify(|existing| {
                if reason == MintReason::Relation {
                    *existing = MintReason::Relation;
                }
            })
            .or_insert(reason);
    }
}

struct EdgeSource {
    path: PathBuf,
    loc: TaskRef,
    label: Option<String>,
}

struct MentionSource {
    path: PathBuf,
    loc: TaskRef,
}

#[derive(Default)]
struct DependencyState {
    depends_from_loc: BTreeMap<TaskRef, BTreeSet<TaskIdentity>>,
    depends_identities: BTreeSet<(TaskIdentity, TaskIdentity)>,
    malformed_depends: BTreeSet<TaskIdentity>,
}

impl RelationsPass<'_> {
    fn run(&mut self) {
        let mut collected = self.collect_edges();
        self.collect_mentions(&mut collected);
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
            mention_by_target: BTreeMap::new(),
            uncertain: BTreeSet::new(),
            mention_uncertain: BTreeSet::new(),
            malformed_depends: BTreeSet::new(),
            mint_needed: BTreeMap::new(),
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
        let contents_ref = if archive {
            self.archives[file_index].contents.as_str()
        } else {
            self.files[file_index].contents.as_str()
        };
        if !admits_relation_scan(contents_ref, self.mentions_enabled) {
            return;
        }
        let relative = if archive {
            self.archives[file_index].relative_path.clone()
        } else {
            self.files[file_index].relative_path.clone()
        };
        let tasks = if archive {
            self.archives[file_index].tasks.clone()
        } else {
            self.files[file_index].tasks.clone()
        };
        let duplicates = duplicate_block_ids(&tasks);
        for task in &tasks {
            if let Some(block_id) = &task.block_id
                && duplicates.contains(block_id)
            {
                collected
                    .uncertain
                    .insert((relative.clone(), block_id.clone()));
            }
        }
        if !contents_ref.contains("LINKS") {
            return;
        }
        let contents = contents_ref.to_string();
        let lines = logical_lines(&contents);
        let line_refs: Vec<&str> = lines.to_vec();
        let fenced = fenced_lines(&line_refs, 0..line_refs.len());
        for (task_index, task) in tasks.iter().enumerate() {
            let duplicated = task
                .block_id
                .as_ref()
                .is_some_and(|id| duplicates.contains(id));
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
            if duplicated && let Some(block_id) = &task.block_id {
                self.outcome.warnings.push(RelationWarning {
                        kind: "duplicate_block_id".to_string(),
                        path: display_path(&relative),
                        line: task.line_index + 1,
                        message: format!(
                            "note has more than one ^{block_id}; source not listed this run"
                        ),
                    });
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
                        collected.note_mint(
                            file_index,
                            task_index_of(loc),
                            MintReason::Relation,
                        );
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

    fn mint_sources(&mut self, needed: &BTreeMap<(usize, usize), MintReason>) {
        let pending: Vec<_> = needed
            .iter()
            .map(|(&(file_index, task_index), &reason)| {
                (file_index, task_index, reason)
            })
            .collect();
        for (file_index, task_index, reason) in pending {
            if self.files[file_index].tasks[task_index].block_id.is_some() {
                continue;
            }
            self.mint_one(file_index, task_index, reason);
        }
    }

    fn mint_one(
        &mut self,
        file_index: usize,
        task_index: usize,
        reason: MintReason,
    ) {
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
            reason: Some(reason.as_str().to_string()),
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
            for target in collected
                .by_target
                .keys()
                .chain(collected.mention_by_target.keys())
            {
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
            let line_refs: Vec<&str> = lines.to_vec();
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
        self.outcome.mention_edges += desired
            .iter()
            .filter(|(_, label)| is_mention_label(label))
            .count();
        let current = current_linked_from_entries(&children);
        let mentions_added = mention_link_delta(&desired, &current);
        let mentions_removed = mention_link_delta(&current, &desired);
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
                    mentions_added,
                    mentions_removed,
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
        if self.mentions_enabled
            && !collected.malformed_depends.contains(&identity)
            && let Some(sources) = collected.mention_by_target.get(&identity)
        {
            for source in sources {
                let Some(block_id) = self.source_block_id(source.loc) else {
                    continue;
                };
                let key = (source.path.clone(), block_id);
                if self.source_is_mention_uncertain(collected, &key)
                    || desired.contains_key(&key)
                {
                    continue;
                }
                let link = canonical_link(
                    &source.path,
                    &key.1,
                    owner_path,
                    self.note_index,
                );
                desired.insert(key, (link, Some(MENTION_LABEL.to_string())));
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
                let keep_uncertain_relation =
                    collected.uncertain.contains(&key)
                        && entry.kind != RelationEntryKind::Mention;
                let keep_uncertain_mention = self.mentions_enabled
                    && entry.kind == RelationEntryKind::Mention
                    && (self.source_is_mention_uncertain(collected, &key)
                        || collected.malformed_depends.contains(&identity));
                if keep_uncertain_relation || keep_uncertain_mention {
                    let link = canonical_link(
                        &path,
                        &entry.block_id,
                        owner_path,
                        self.note_index,
                    );
                    desired.entry(key).or_insert((link, entry.storage_label()));
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

    fn source_is_mention_uncertain(
        &self,
        collected: &CollectedEdges,
        key: &TaskIdentity,
    ) -> bool {
        collected.uncertain.contains(key)
            || collected.mention_uncertain.contains(key)
    }

    fn mark_mention_file_uncertainty(&self, collected: &mut CollectedEdges) {
        for file in self.files.iter().chain(self.archives.iter()) {
            let lines = logical_lines(&file.contents);
            let refs: Vec<&str> = lines.to_vec();
            if !has_unclosed_fence(&refs) {
                continue;
            }
            for task in &file.tasks {
                if let Some(id) = &task.block_id {
                    collected
                        .mention_uncertain
                        .insert((file.relative_path.clone(), id.clone()));
                }
            }
        }
    }

    fn collect_mentions(&mut self, collected: &mut CollectedEdges) {
        if !self.enabled || !self.mentions_enabled {
            return;
        }
        self.mark_mention_file_uncertainty(collected);
        let deps = self.collect_dependency_state();
        collected.malformed_depends = deps.malformed_depends.clone();
        let mention_index = self.note_index.with_additional(
            self.archives.iter().map(|file| file.relative_path.clone()),
        );
        let scan = scan_mentions(
            self.files.iter().chain(self.archives.iter()),
            &mention_index,
        );
        for pair in scan.pairs {
            self.record_mention_pair(pair, &deps, collected);
        }
    }

    fn collect_dependency_state(&self) -> DependencyState {
        let mut state = DependencyState::default();
        let active_count = self.files.len();
        for file_index in 0..active_count {
            self.collect_file_dependencies(file_index, false, &mut state);
        }
        let archive_count = self.archives.len();
        for file_index in 0..archive_count {
            self.collect_file_dependencies(file_index, true, &mut state);
        }
        state
    }

    fn collect_file_dependencies(
        &self,
        file_index: usize,
        archive: bool,
        state: &mut DependencyState,
    ) {
        let contents_ref = if archive {
            self.archives[file_index].contents.as_str()
        } else {
            self.files[file_index].contents.as_str()
        };
        if !contents_ref.contains("DEPENDS ON") {
            return;
        }
        let relative = if archive {
            self.archives[file_index].relative_path.clone()
        } else {
            self.files[file_index].relative_path.clone()
        };
        let contents = contents_ref.to_string();
        let tasks = if archive {
            self.archives[file_index].tasks.clone()
        } else {
            self.files[file_index].tasks.clone()
        };
        let lines = logical_lines(&contents);
        let line_refs: Vec<&str> = lines.to_vec();
        let fenced = fenced_lines(&line_refs, 0..line_refs.len());
        for (task_index, task) in tasks.iter().enumerate() {
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
            let Some(child) =
                dependency_child_of(&line_refs, &fenced, task.line_index)
            else {
                continue;
            };
            match child.parsed {
                DependencyLine::Malformed => {
                    if let Some(id) = &task.block_id {
                        state
                            .malformed_depends
                            .insert((relative.clone(), id.clone()));
                    }
                }
                DependencyLine::Accepted { links, .. } => {
                    for link in links {
                        let Some(target_path) =
                            self.resolve_target_path(&relative, &link.target)
                        else {
                            continue;
                        };
                        let target = (target_path, link.block_id);
                        state
                            .depends_from_loc
                            .entry(loc)
                            .or_default()
                            .insert(target.clone());
                        if let Some(id) = &task.block_id {
                            state.depends_identities.insert((
                                (relative.clone(), id.clone()),
                                target,
                            ));
                        }
                    }
                }
                DependencyLine::Empty | DependencyLine::NotALine => {}
            }
        }
    }

    fn record_mention_pair(
        &mut self,
        pair: MentionPair,
        deps: &DependencyState,
        collected: &mut CollectedEdges,
    ) {
        let Some(loc) = self.lookup_loc(&pair.source_path, pair.source_line)
        else {
            return;
        };
        let target = (pair.target_path.clone(), pair.target_block_id.clone());
        if source_has_relation(collected, loc, &target)
            || dependency_joins(loc, &pair, deps, &target)
        {
            return;
        }
        if pair.source_uncertain {
            if let Some(id) = &pair.source_block_id {
                collected
                    .mention_uncertain
                    .insert((pair.source_path.clone(), id.clone()));
            }
            return;
        }
        if deps.malformed_depends.contains(&target) {
            return;
        }
        let (archive, file_index) = match loc {
            TaskRef::Active { file, .. } => (false, file),
            TaskRef::Archive { file, .. } => (true, file),
        };
        if pair.source_block_id.is_none() {
            let mintable = !archive
                && self.writable(file_index)
                && !self.quiet_deferred.contains(&file_index);
            if mintable {
                collected.note_mint(
                    file_index,
                    task_index_of(loc),
                    MintReason::Mention,
                );
            } else {
                return;
            }
        }
        collected.mention_by_target.entry(target).or_default().push(
            MentionSource {
                path: pair.source_path,
                loc,
            },
        );
    }

    fn lookup_loc(&self, path: &Path, line: usize) -> Option<TaskRef> {
        for (file_index, file) in self.files.iter().enumerate() {
            if file.relative_path != path {
                continue;
            }
            for (task_index, task) in file.tasks.iter().enumerate() {
                if task.line_index == line {
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
                if task.line_index == line {
                    return Some(TaskRef::Archive {
                        file: file_index,
                        task: task_index,
                    });
                }
            }
        }
        None
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

fn admits_relation_scan(contents: &str, mentions_enabled: bool) -> bool {
    contents.contains("LINKS")
        || (mentions_enabled && admits_mention_bytes(contents))
}

fn admits_mention_bytes(contents: &str) -> bool {
    contents.contains("#task") && contents.contains("#^")
}

fn is_mention_label(label: &Option<String>) -> bool {
    label.as_deref() == Some(MENTION_LABEL)
}

fn current_linked_from_entries(
    children: &[super::super::task_relations::parse::LinkedFromChild],
) -> BTreeSet<(String, Option<String>)> {
    let mut current = BTreeSet::new();
    for child in children {
        let LinkedFromLine::Accepted { entries, .. } = &child.parsed else {
            continue;
        };
        for entry in entries {
            current.insert((entry.canonical_text(), entry.storage_label()));
        }
    }
    current
}

fn mention_link_delta(
    from: &BTreeSet<(String, Option<String>)>,
    subtract: &BTreeSet<(String, Option<String>)>,
) -> Vec<String> {
    from.iter()
        .filter(|entry| {
            is_mention_label(&entry.1) && !subtract.contains(*entry)
        })
        .map(|(link, _)| link.clone())
        .collect()
}

fn source_has_relation(
    collected: &CollectedEdges,
    loc: TaskRef,
    target: &TaskIdentity,
) -> bool {
    collected
        .by_target
        .get(target)
        .is_some_and(|sources| sources.iter().any(|source| source.loc == loc))
}

fn dependency_joins(
    loc: TaskRef,
    pair: &MentionPair,
    deps: &DependencyState,
    target: &TaskIdentity,
) -> bool {
    if deps
        .depends_from_loc
        .get(&loc)
        .is_some_and(|targets| targets.contains(target))
    {
        return true;
    }
    let Some(id) = &pair.source_block_id else {
        return false;
    };
    deps.depends_identities
        .contains(&(target.clone(), (pair.source_path.clone(), id.clone())))
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

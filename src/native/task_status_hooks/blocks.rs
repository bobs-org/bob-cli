//! BLOCKS-line projection pass (`docs/task-dependencies.md` §14.6).
//!
//! Runs immediately after R1–R10 re-parse. It never reads BLOCKS lines
//! as dependency input: desired membership comes from the accepted
//! explicit edges, plus existing entries that resolve to uncertain
//! (R10) dependents so those are never pruned.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use super::super::capture_block_ids::mint_block_id;
use super::super::collect_done::block_ids_in_markdown;
use super::super::task_dependencies::blocks::{
    blocks_children_of, plan_blocks_line, BlocksEdit, BlocksLine,
    BlocksLinePlan,
};
use super::super::task_dependencies::format::{
    canonical_link, has_final_newline, note_line_ending,
};
use super::super::vault_links::NoteIndex;
use super::*;

/// Outcome of one BLOCKS projection pass: content edits already
/// applied to `files`, plus the additive report rows.
pub(super) struct BlocksOutcome {
    pub(super) touched_files: BTreeSet<usize>,
    pub(super) original_contents: BTreeMap<usize, String>,
    pub(super) updates: Vec<BlocksLineUpdate>,
    pub(super) mints: Vec<MintedDependentBlockId>,
    pub(super) warnings: Vec<DependencyWarning>,
}

/// Project BLOCKS lines from the accepted explicit edges.
///
/// `files` is mutated in place (note contents only). Callers must
/// re-parse touched files afterwards. `previous_daily_file` is never
/// written. Dependents whose mint would land in `quiet_deferred` are
/// skipped this run (BR5).
#[allow(clippy::too_many_arguments)]
pub(super) fn reconcile_blocks_lines(
    files: &mut [FileScan],
    note_index: &NoteIndex,
    settings: &TasksSettings,
    edges: &BTreeMap<(usize, usize), BTreeSet<(PathBuf, String)>>,
    uncertain: &BTreeSet<(usize, usize)>,
    previous_daily_file: Option<usize>,
    quiet_deferred: &BTreeSet<usize>,
    enabled: bool,
) -> BlocksOutcome {
    let mut pass = BlocksPass {
        files,
        note_index,
        settings,
        edges,
        uncertain,
        previous_daily_file,
        quiet_deferred,
        enabled,
        outcome: BlocksOutcome {
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

struct BlocksPass<'a> {
    files: &'a mut [FileScan],
    note_index: &'a NoteIndex,
    settings: &'a TasksSettings,
    edges: &'a BTreeMap<(usize, usize), BTreeSet<(PathBuf, String)>>,
    uncertain: &'a BTreeSet<(usize, usize)>,
    previous_daily_file: Option<usize>,
    quiet_deferred: &'a BTreeSet<usize>,
    enabled: bool,
    outcome: BlocksOutcome,
    used: BTreeMap<usize, HashSet<String>>,
}

impl BlocksPass<'_> {
    fn run(&mut self) {
        let owners = self.collect_owners();
        if self.enabled {
            self.mint_dependents(&owners);
        }
        let mut planned: BTreeMap<usize, Vec<BlocksEdit>> = BTreeMap::new();
        for owner in &owners {
            if let Some((file_index, edits)) = self.plan_owner(*owner) {
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

    fn open_task(task: &TaskLine) -> bool {
        task.status_recognized && task.status_type.is_open()
    }

    fn collect_owners(&self) -> BTreeSet<(usize, usize)> {
        let by_identity = identity_lookup(self.files);
        let mut owners = BTreeSet::new();
        for ((d_file, d_task), targets) in self.edges {
            if !self.writable(*d_file) {
                continue;
            }
            let Some(dependent) = self
                .files
                .get(*d_file)
                .and_then(|file| file.tasks.get(*d_task))
            else {
                continue;
            };
            if !Self::open_task(dependent) {
                continue;
            }
            for target in targets {
                let Some(&(t_file, t_task)) = by_identity.get(target) else {
                    continue;
                };
                if !self.writable(t_file) {
                    continue;
                }
                let Some(owner) = self
                    .files
                    .get(t_file)
                    .and_then(|file| file.tasks.get(t_task))
                else {
                    continue;
                };
                if Self::open_task(owner) {
                    owners.insert((t_file, t_task));
                }
            }
        }
        for (file_index, file) in self.files.iter().enumerate() {
            if !self.writable(file_index) {
                continue;
            }
            if !file.contents.contains("BLOCKS")
                && !file.contents.contains("BLOCKING")
            {
                continue;
            }
            let lines = logical_lines(&file.contents);
            let fenced = fenced_lines(&lines, 0..lines.len());
            for (task_index, task) in file.tasks.iter().enumerate() {
                if !Self::open_task(task) {
                    continue;
                }
                if !blocks_children_of(&lines, &fenced, task.line_index)
                    .is_empty()
                {
                    owners.insert((file_index, task_index));
                }
            }
        }
        owners
    }

    fn mint_dependents(&mut self, owners: &BTreeSet<(usize, usize)>) {
        let by_identity = identity_lookup(self.files);
        let mut needed = BTreeSet::new();
        for ((d_file, d_task), targets) in self.edges {
            if !self.writable(*d_file)
                || self.quiet_deferred.contains(d_file)
                || self.uncertain.contains(&(*d_file, *d_task))
            {
                continue;
            }
            let Some(task) = self
                .files
                .get(*d_file)
                .and_then(|file| file.tasks.get(*d_task))
            else {
                continue;
            };
            if task.block_id.is_some() || !Self::open_task(task) {
                continue;
            }
            let lists_an_owner = targets.iter().any(|target| {
                by_identity
                    .get(target)
                    .is_some_and(|owner| owners.contains(owner))
            });
            if lists_an_owner {
                needed.insert((*d_file, *d_task));
            }
        }
        for (file_index, task_index) in needed {
            self.mint_one(file_index, task_index);
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
            reason: None,
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

    fn plan_owner(
        &mut self,
        owner: (usize, usize),
    ) -> Option<(usize, Vec<BlocksEdit>)> {
        let (file_index, task_index) = owner;
        let relative = self.files[file_index].relative_path.clone();
        let task = self.files[file_index].tasks[task_index].clone();
        if !self.writable(file_index) || !Self::open_task(&task) {
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
        let children = blocks_children_of(&line_refs, &fenced, task.line_index);
        if children
            .iter()
            .any(|child| matches!(child.parsed, BlocksLine::Malformed))
        {
            let line = children
                .iter()
                .find(|child| matches!(child.parsed, BlocksLine::Malformed))
                .map(|child| child.line_index + 1)
                .unwrap_or(task.line_index + 1);
            self.outcome.warnings.push(DependencyWarning {
                kind: "malformed_blocks_line".to_string(),
                path: display_path(&relative),
                line,
                detail: "BLOCKS line has trailing prose, a half-typed link, or other residue; left alone".to_string(),
            });
            return None;
        }
        let desired = self.desired_links(&relative, &task, &children);
        match plan_blocks_line(&line_refs, &fenced, task.line_index, &desired) {
            BlocksLinePlan::Noop => None,
            BlocksLinePlan::Malformed => {
                self.outcome.warnings.push(DependencyWarning {
                    kind: "malformed_blocks_line".to_string(),
                    path: display_path(&relative),
                    line: task.line_index + 1,
                    detail: "BLOCKS line has trailing prose, a half-typed link, or other residue; left alone".to_string(),
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
                    mentions_added: Vec::new(),
                    mentions_removed: Vec::new(),
                });
                Some((file_index, edits))
            }
        }
    }

    fn desired_links(
        &self,
        owner_path: &Path,
        owner: &TaskLine,
        children: &[super::super::task_dependencies::blocks::BlocksChild],
    ) -> BTreeSet<String> {
        if !self.enabled {
            return BTreeSet::new();
        }
        let by_identity = identity_lookup(self.files);
        let owner_identity = owner
            .block_id
            .as_ref()
            .map(|block| (owner_path.to_path_buf(), block.clone()));
        let mut desired = BTreeSet::new();
        for ((d_file, d_task), targets) in self.edges {
            if self.uncertain.contains(&(*d_file, *d_task))
                || !self.writable(*d_file)
            {
                continue;
            }
            let Some(target_identity) = &owner_identity else {
                continue;
            };
            if !targets.contains(target_identity) {
                continue;
            }
            let Some(dependent) = self
                .files
                .get(*d_file)
                .and_then(|file| file.tasks.get(*d_task))
            else {
                continue;
            };
            if !Self::open_task(dependent) {
                continue;
            }
            // BR5: a dependent that still needs a mint is not listed.
            // Quiet-deferred notes skip that mint above.
            let Some(block_id) = &dependent.block_id else {
                continue;
            };
            let link = canonical_link(
                &self.files[*d_file].relative_path,
                block_id,
                owner_path,
                self.note_index,
            );
            desired.insert(link);
        }
        for child in children {
            let BlocksLine::Accepted { links, .. } = &child.parsed else {
                continue;
            };
            for link in links {
                let Some(resolved) =
                    resolve_blocks_identity(link, owner_path, self.note_index)
                else {
                    continue;
                };
                let Some(&(d_file, d_task)) = by_identity.get(&resolved) else {
                    continue;
                };
                if self.uncertain.contains(&(d_file, d_task)) {
                    desired.insert(canonical_link(
                        &resolved.0,
                        &resolved.1,
                        owner_path,
                        self.note_index,
                    ));
                }
            }
        }
        desired
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

fn identity_lookup(
    files: &[FileScan],
) -> BTreeMap<(PathBuf, String), (usize, usize)> {
    let mut lookup = BTreeMap::new();
    for (file_index, file) in files.iter().enumerate() {
        for (task_index, task) in file.tasks.iter().enumerate() {
            if let Some(block_id) = &task.block_id {
                lookup
                    .entry((file.relative_path.clone(), block_id.clone()))
                    .or_insert((file_index, task_index));
            }
        }
    }
    lookup
}

fn resolve_blocks_identity(
    link: &super::super::task_dependencies::blocks::BlocksLink,
    owner_path: &Path,
    note_index: &NoteIndex,
) -> Option<(PathBuf, String)> {
    let path = note_index.resolve(Some(owner_path), &link.target)?;
    Some((path, link.block_id.clone()))
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

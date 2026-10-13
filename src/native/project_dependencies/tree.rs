//! Project parent/child relationships used by cascade blocking
//! (`docs/task-dependencies.md` §13.1, §13.3).

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
};

use super::super::{
    projects::{frontmatter_value, parse_frontmatter, wikilink_path_target},
    task_status_hooks::FileScan,
    vault_links::NoteIndex,
};
use super::blocking::{ProjectBlockReasons, ProjectContext};

const MAX_PROJECT_DEPTH: usize = 16;

/// Read the scalar `parent:` wikilink the same way project sync reads it,
/// preserving the target path while dropping an alias and heading.
pub(crate) fn project_parent_target(contents: &str) -> Option<String> {
    let frontmatter = parse_frontmatter(contents)?;
    let value = frontmatter_value(&frontmatter, "parent")?;
    wikilink_path_target(value)
}

/// Resolve a project's scalar `parent:` target through the scanned-note
/// index. A self-parent is invalid and treated as no parent (PDS6).
pub(crate) fn resolve_project_parent(
    child_rel: &Path,
    contents: &str,
    note_index: &NoteIndex,
) -> Option<PathBuf> {
    let target = project_parent_target(contents)?;
    let parent = note_index.resolve(Some(child_rel), &target)?;
    (parent != child_rel).then_some(parent)
}

/// Follow scalar `parent:` links upward, reading each parent only as needed.
/// The first note's contents are supplied by the caller; cycles, self-links,
/// unresolved parents, and walks deeper than the project limit stop cleanly.
/// Project recognition remains with the caller, which has the task settings
/// needed to apply §13.1's exact `project(T)` predicate.
pub(crate) fn ancestor_chain<F>(
    child_rel: &Path,
    contents: &str,
    note_index: &NoteIndex,
    mut read: F,
    max_depth: usize,
) -> Vec<PathBuf>
where
    F: FnMut(&Path) -> Option<String>,
{
    let mut ancestors = Vec::new();
    let mut visited = BTreeSet::from([child_rel.to_path_buf()]);
    let mut current_path = child_rel.to_path_buf();
    let mut current_contents = contents.to_string();
    for _ in 0..max_depth.min(MAX_PROJECT_DEPTH) {
        let Some(parent) = resolve_project_parent(
            &current_path,
            &current_contents,
            note_index,
        ) else {
            break;
        };
        if !visited.insert(parent.clone()) {
            break;
        }
        ancestors.push(parent.clone());
        let Some(parent_contents) = read(&parent) else {
            break;
        };
        current_path = parent;
        current_contents = parent_contents;
    }
    ancestors
}

/// Recognised-project ancestors of `child_rel`, nearest first. The walk
/// follows [`ancestor_chain`] and stops at the first note that is not a
/// project (PDS6, PDS7).
pub(crate) fn project_ancestors<F, P>(
    child_rel: &Path,
    contents: &str,
    note_index: &NoteIndex,
    mut read: F,
    mut is_project: P,
) -> Vec<PathBuf>
where
    F: FnMut(&Path) -> Option<String>,
    P: FnMut(&Path, &str) -> bool,
{
    let chain = ancestor_chain(
        child_rel,
        contents,
        note_index,
        &mut read,
        MAX_PROJECT_DEPTH,
    );
    let mut ancestors = Vec::new();
    for ancestor in chain {
        let Some(ancestor_contents) = read(&ancestor) else {
            break;
        };
        if !is_project(&ancestor, &ancestor_contents) {
            break;
        }
        ancestors.push(ancestor);
    }
    ancestors
}

/// One vault-wide project hierarchy and its inherited blocking reasons.
/// The hierarchy is built from the same scanned files and NoteIndex as
/// reconcile, so resolving a parent adds no vault reads.
#[derive(Debug, Clone)]
pub(crate) struct ProjectTree {
    path_indexes: BTreeMap<PathBuf, usize>,
    // Shared parent/child indexes consumed by the follow-on Rust phases.
    #[allow(dead_code)]
    parent: Vec<Option<usize>>,
    #[allow(dead_code)]
    children: Vec<Vec<usize>>,
    ancestors: Vec<Vec<usize>>,
    descendants: Vec<Vec<usize>>,
    contexts: Vec<ProjectContext>,
    project_task_indexes: Vec<Option<usize>>,
    own_reasons: Vec<ProjectBlockReasons>,
    inherited: Vec<Vec<(PathBuf, ProjectBlockReasons)>>,
    cycles: Vec<Vec<usize>>,
}

impl ProjectTree {
    /// Build the topology once for a reconcile run. Reasons may be filled
    /// after R1–R10 has reconciled the Depends-On fields via
    /// `set_project_reasons`.
    pub(crate) fn build(
        files: &[FileScan],
        contexts: &[ProjectContext],
        reasons: &[ProjectBlockReasons],
        note_index: &NoteIndex,
    ) -> Self {
        debug_assert_eq!(files.len(), contexts.len());
        debug_assert_eq!(files.len(), reasons.len());

        let path_indexes = files
            .iter()
            .enumerate()
            .map(|(index, file)| (file.relative_path.clone(), index))
            .collect::<BTreeMap<_, _>>();
        let project_task_indexes = contexts
            .iter()
            .map(ProjectContext::project_task_index)
            .collect::<Vec<_>>();
        let mut parent = vec![None; files.len()];
        for (index, file) in files.iter().enumerate() {
            if !contexts[index].recognised {
                continue;
            }
            parent[index] = resolve_project_parent(
                &file.relative_path,
                &file.contents,
                note_index,
            )
            .and_then(|path| path_indexes.get(&path).copied())
            .filter(|parent_index| *parent_index != index);
        }

        let mut children = vec![Vec::new(); files.len()];
        for (child_index, parent_index) in parent.iter().enumerate() {
            let Some(parent_index) = parent_index else {
                continue;
            };
            if contexts[*parent_index].recognised {
                children[*parent_index].push(child_index);
            }
        }
        for child_indexes in &mut children {
            child_indexes.sort_by(|left, right| {
                files[*left].relative_path.cmp(&files[*right].relative_path)
            });
        }

        let mut ancestors = vec![Vec::new(); files.len()];
        for index in 0..files.len() {
            if !contexts[index].recognised {
                continue;
            }
            let mut seen = BTreeSet::from([index]);
            let mut current = parent[index];
            for _ in 0..MAX_PROJECT_DEPTH {
                let Some(parent_index) = current else {
                    break;
                };
                if !contexts[parent_index].recognised
                    || !seen.insert(parent_index)
                {
                    break;
                }
                ancestors[index].push(parent_index);
                current = parent[parent_index];
            }
        }

        let mut descendants = vec![Vec::new(); files.len()];
        for root in 0..files.len() {
            if !contexts[root].recognised {
                continue;
            }
            let mut queue = VecDeque::from([(root, 0usize)]);
            let mut seen = BTreeSet::from([root]);
            while let Some((parent_index, depth)) = queue.pop_front() {
                if depth == MAX_PROJECT_DEPTH {
                    continue;
                }
                for child_index in &children[parent_index] {
                    if seen.insert(*child_index) {
                        descendants[root].push(*child_index);
                        queue.push_back((*child_index, depth + 1));
                    }
                }
            }
        }

        let cycles = find_cycles(files, contexts, &parent);
        let mut tree = Self {
            path_indexes,
            parent,
            children,
            ancestors,
            descendants,
            contexts: contexts.to_vec(),
            project_task_indexes,
            own_reasons: reasons.to_vec(),
            inherited: vec![Vec::new(); files.len()],
            cycles,
        };
        tree.rebuild_inherited(files);
        tree
    }

    /// Replace own project reasons after dependency fields have been
    /// reconciled, then refresh each project's nearest-first inheritance.
    pub(crate) fn set_project_reasons(
        &mut self,
        files: &[FileScan],
        reasons: Vec<ProjectBlockReasons>,
    ) {
        debug_assert_eq!(files.len(), self.own_reasons.len());
        debug_assert_eq!(files.len(), reasons.len());
        self.own_reasons = reasons;
        self.rebuild_inherited(files);
    }

    fn rebuild_inherited(&mut self, files: &[FileScan]) {
        self.inherited = vec![Vec::new(); files.len()];
        for file_index in 0..files.len() {
            for ancestor_index in &self.ancestors[file_index] {
                let reasons = &self.own_reasons[*ancestor_index];
                if !reasons.is_empty() {
                    self.inherited[file_index].push((
                        files[*ancestor_index].relative_path.clone(),
                        reasons.clone(),
                    ));
                }
            }
        }
    }

    pub(crate) fn own_reasons(
        &self,
        file_index: usize,
    ) -> &ProjectBlockReasons {
        &self.own_reasons[file_index]
    }

    /// Owning ancestor plus its reasons, nearest first.
    pub(crate) fn inherited(
        &self,
        file_index: usize,
    ) -> &[(PathBuf, ProjectBlockReasons)] {
        &self.inherited[file_index]
    }

    pub(crate) fn ancestors(&self, file_index: usize) -> &[usize] {
        &self.ancestors[file_index]
    }

    pub(crate) fn descendants(&self, file_index: usize) -> &[usize] {
        &self.descendants[file_index]
    }

    pub(crate) fn cycles(&self) -> &[Vec<usize>] {
        &self.cycles
    }

    pub(crate) fn project_task_index(
        &self,
        file_index: usize,
    ) -> Option<usize> {
        self.project_task_indexes[file_index]
    }

    pub(crate) fn is_project_task(
        &self,
        file_index: usize,
        task: &super::super::task_status_hooks::TaskLine,
    ) -> bool {
        self.contexts[file_index].is_project_task(task)
    }

    pub(crate) fn is_resident(
        &self,
        file_index: usize,
        task: &super::super::task_status_hooks::TaskLine,
    ) -> bool {
        self.contexts[file_index].is_resident(task)
    }

    #[allow(dead_code)] // Shared by the follow-on Rust phases.
    pub(crate) fn parent_index(&self, file_index: usize) -> Option<usize> {
        self.parent[file_index]
    }

    #[allow(dead_code)] // Shared by the follow-on Rust phases.
    pub(crate) fn children(&self, file_index: usize) -> &[usize] {
        &self.children[file_index]
    }

    /// Whether an open project's `^prj` depends on itself or on any of its
    /// own descendant notes. Such targets stay on the line but never project.
    pub(crate) fn is_internal_target(
        &self,
        source_file: usize,
        target_path: &Path,
    ) -> bool {
        let Some(target_file) = self.path_indexes.get(target_path) else {
            return false;
        };
        *target_file == source_file
            || self.descendants[source_file].contains(target_file)
    }

    /// Remove dependency ids whose task lives in a descendant note.
    pub(crate) fn filter_descendant_ids(
        &self,
        file_index: usize,
        files: &[FileScan],
        ids: &[String],
    ) -> Vec<String> {
        if ids.is_empty() || self.descendants(file_index).is_empty() {
            return ids.to_vec();
        }
        let internal = self
            .descendants(file_index)
            .iter()
            .flat_map(|descendant| files[*descendant].tasks.iter())
            .filter_map(|task| task.task_id.as_deref())
            .collect::<BTreeSet<_>>();
        ids.iter()
            .filter(|id| !internal.contains(id.as_str()))
            .cloned()
            .collect()
    }
}

fn find_cycles(
    files: &[FileScan],
    contexts: &[ProjectContext],
    parent: &[Option<usize>],
) -> Vec<Vec<usize>> {
    let mut cycles = BTreeMap::<Vec<PathBuf>, Vec<usize>>::new();
    for start in 0..files.len() {
        if !contexts[start].recognised {
            continue;
        }
        let mut order = Vec::<usize>::new();
        let mut positions = BTreeMap::<usize, usize>::new();
        let mut current = Some(start);
        for _ in 0..=MAX_PROJECT_DEPTH {
            let Some(index) = current else {
                break;
            };
            if !contexts[index].recognised {
                break;
            }
            if let Some(position) = positions.get(&index).copied() {
                let mut cycle = order[position..].to_vec();
                cycle.sort_by(|left, right| {
                    files[*left].relative_path.cmp(&files[*right].relative_path)
                });
                let key = cycle
                    .iter()
                    .map(|member| files[*member].relative_path.clone())
                    .collect();
                cycles.entry(key).or_insert(cycle);
                break;
            }
            positions.insert(index, order.len());
            order.push(index);
            current = parent[index];
        }
    }
    cycles.into_values().collect()
}

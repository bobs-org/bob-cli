//! Shared relation-target resolution (`docs/task-relations.md` SU3).
//!
//! Reconcile and the relation-successor classifier resolve LINKS
//! targets through this helper so same-note, basename, qualified-path,
//! archive, task-vs-nontask, and self-edge rules stay in one place.

use std::path::{Path, PathBuf};

use super::super::task_dependencies::is_archive_path;
use super::super::vault_links::{target_to_markdown_path, NoteIndex};
use super::RelationEntry;

/// What lives at a resolved `(path, block id)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RelationTargetKind {
    Task,
    NonTask,
    Missing,
}

/// Why a LINKS entry failed to resolve to a different task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RelationResolveError {
    Unresolved,
    SelfRelation,
    TargetNotTask,
}

impl RelationResolveError {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Unresolved => "unresolved_relation_link",
            Self::SelfRelation => "self_relation",
            Self::TargetNotTask => "relation_target_not_task",
        }
    }
}

/// Path and identity lookup used by [`resolve_relation_entry`].
pub(crate) trait RelationResolveIndex {
    fn resolve_note_path(
        &self,
        source_path: &Path,
        target: &str,
    ) -> Option<PathBuf>;

    fn target_kind(&self, path: &Path, block_id: &str) -> RelationTargetKind;
}

/// Resolve a LINKS target string to a vault-relative note path.
///
/// Empty targets are the source note. Explicit `done/` paths resolve
/// only when that archive file is known; they never fall back to a
/// basename. Other targets use [`NoteIndex`] exact-path then unique
/// basename resolution, so ambiguous basenames stay unresolved.
pub(crate) fn resolve_relation_target_path(
    source_path: &Path,
    target: &str,
    notes: &NoteIndex,
    archive_known: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    if target.is_empty() {
        return Some(source_path.to_path_buf());
    }
    let candidate = target_to_markdown_path(target)?;
    if is_archive_path(&candidate) {
        return archive_known(&candidate).then_some(candidate);
    }
    notes.resolve(Some(source_path), target)
}

/// Resolve one LINKS entry to `(path, block_id)` or a reconcile warning
/// kind. Self-edges compare the source task's block id to the entry.
pub(crate) fn resolve_relation_entry(
    source_path: &Path,
    source_block_id: Option<&str>,
    entry: &RelationEntry,
    index: &impl RelationResolveIndex,
) -> Result<(PathBuf, String), RelationResolveError> {
    let Some(path) = index.resolve_note_path(source_path, &entry.target) else {
        return Err(RelationResolveError::Unresolved);
    };
    if source_block_id == Some(entry.block_id.as_str()) && path == source_path {
        return Err(RelationResolveError::SelfRelation);
    }
    match index.target_kind(&path, &entry.block_id) {
        RelationTargetKind::Task => Ok((path, entry.block_id.clone())),
        RelationTargetKind::NonTask => Err(RelationResolveError::TargetNotTask),
        RelationTargetKind::Missing => Err(RelationResolveError::Unresolved),
    }
}

/// In-memory index for parity tests of the shared resolver.
#[cfg(test)]
pub(crate) struct MemoryRelationIndex<'a> {
    pub notes: &'a NoteIndex,
    pub archive_paths: &'a [PathBuf],
    pub tasks: &'a [(PathBuf, String)],
    pub ids: &'a [(PathBuf, String)],
}

#[cfg(test)]
impl RelationResolveIndex for MemoryRelationIndex<'_> {
    fn resolve_note_path(
        &self,
        source_path: &Path,
        target: &str,
    ) -> Option<PathBuf> {
        resolve_relation_target_path(source_path, target, self.notes, |path| {
            self.archive_paths.iter().any(|known| known == path)
        })
    }

    fn target_kind(&self, path: &Path, block_id: &str) -> RelationTargetKind {
        if self
            .tasks
            .iter()
            .any(|(note, id)| note == path && id == block_id)
        {
            return RelationTargetKind::Task;
        }
        if self
            .ids
            .iter()
            .any(|(note, id)| note == path && id == block_id)
        {
            return RelationTargetKind::NonTask;
        }
        RelationTargetKind::Missing
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::task_relations::{RelationEntry, RelationEntryKind};

    fn entry(target: &str, block_id: &str) -> RelationEntry {
        RelationEntry {
            target: target.to_string(),
            block_id: block_id.to_string(),
            label: Some("starts after".to_string()),
            kind: RelationEntryKind::Relation,
            embedded: false,
            struck: false,
            aliased: false,
        }
    }

    fn index<'a>(
        notes: &'a NoteIndex,
        archive_paths: &'a [PathBuf],
        tasks: &'a [(PathBuf, String)],
        ids: &'a [(PathBuf, String)],
    ) -> MemoryRelationIndex<'a> {
        MemoryRelationIndex {
            notes,
            archive_paths,
            tasks,
            ids,
        }
    }

    #[test]
    fn empty_target_is_the_source_note() {
        let notes = NoteIndex::from_paths([PathBuf::from("agents.md")]);
        let archives = Vec::new();
        let tasks = vec![(PathBuf::from("agents.md"), "implement".to_string())];
        let ids = Vec::new();
        let lookup = index(&notes, &archives, &tasks, &ids);
        let resolved = resolve_relation_entry(
            Path::new("agents.md"),
            Some("evaluate"),
            &entry("", "implement"),
            &lookup,
        )
        .expect("same-note task");
        assert_eq!(resolved.0, PathBuf::from("agents.md"));
        assert_eq!(resolved.1, "implement");
    }

    #[test]
    fn basename_and_qualified_path_resolve_the_same_note() {
        let notes = NoteIndex::from_paths([
            PathBuf::from("Areas/Home.md"),
            PathBuf::from("Projects/Alpha.md"),
        ]);
        assert_eq!(
            resolve_relation_target_path(
                Path::new("inbox.md"),
                "alpha",
                &notes,
                |_| false,
            ),
            Some(PathBuf::from("Projects/Alpha.md"))
        );
        assert_eq!(
            resolve_relation_target_path(
                Path::new("inbox.md"),
                "Projects/Alpha",
                &notes,
                |_| false,
            ),
            Some(PathBuf::from("Projects/Alpha.md"))
        );
    }

    #[test]
    fn ambiguous_basename_stays_unresolved() {
        let notes = NoteIndex::from_paths([
            PathBuf::from("Areas/Home.md"),
            PathBuf::from("Projects/home.md"),
        ]);
        assert_eq!(
            resolve_relation_target_path(
                Path::new("inbox.md"),
                "home",
                &notes,
                |_| false,
            ),
            None
        );
    }

    #[test]
    fn explicit_archive_target_resolves_only_when_known() {
        let notes = NoteIndex::from_paths([PathBuf::from("agents.md")]);
        let known = PathBuf::from("done/agents.md");
        assert_eq!(
            resolve_relation_target_path(
                Path::new("agents.md"),
                "done/agents",
                &notes,
                |path| path == known.as_path(),
            ),
            Some(known.clone())
        );
        assert_eq!(
            resolve_relation_target_path(
                Path::new("agents.md"),
                "done/agents",
                &notes,
                |_| false,
            ),
            None
        );
    }

    #[test]
    fn missing_and_non_task_block_ids_match_reconcile_warnings() {
        let notes = NoteIndex::from_paths([PathBuf::from("agents.md")]);
        let archives = Vec::new();
        let tasks = vec![(PathBuf::from("agents.md"), "implement".to_string())];
        let ids = vec![(PathBuf::from("agents.md"), "heading".to_string())];
        let lookup = index(&notes, &archives, &tasks, &ids);
        let missing = resolve_relation_entry(
            Path::new("agents.md"),
            Some("evaluate"),
            &entry("", "gone"),
            &lookup,
        );
        assert_eq!(missing, Err(RelationResolveError::Unresolved));
        let nontask = resolve_relation_entry(
            Path::new("agents.md"),
            Some("evaluate"),
            &entry("", "heading"),
            &lookup,
        );
        assert_eq!(nontask, Err(RelationResolveError::TargetNotTask));
    }

    #[test]
    fn self_edge_is_rejected_even_when_the_task_exists() {
        let notes = NoteIndex::from_paths([PathBuf::from("agents.md")]);
        let archives = Vec::new();
        let tasks = vec![(PathBuf::from("agents.md"), "evaluate".to_string())];
        let ids = Vec::new();
        let lookup = index(&notes, &archives, &tasks, &ids);
        let error = resolve_relation_entry(
            Path::new("agents.md"),
            Some("evaluate"),
            &entry("", "evaluate"),
            &lookup,
        );
        assert_eq!(error, Err(RelationResolveError::SelfRelation));
    }
}

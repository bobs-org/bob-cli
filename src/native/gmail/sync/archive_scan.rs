//! Strict archive-specific task scan: positive closed-task evidence.
//!
//! Archive eligibility needs proof that every task linking a thread's mail
//! note is closed, with at least one positively closed row (`x`, `X`, or
//! `-` per the `cancelled_archives` decision). This module gathers that
//! evidence across live notes **and** `done/`, reusing the watch scan's
//! placement rules ([`scan`]) and identity resolution ([`MailLinks`]):
//! exact paths then unique basenames, own-line or direct link-only child
//! links, never subject matching.
//!
//! Differences from the watch scan are all strictness:
//!
//! - `done/` is included, so a closed row moved to the archive still
//!   counts and an open row lingering there still vetoes.
//! - Hidden, generated, mail-folder, and dot-directory exclusions,
//!   symlink confinement, and zorg-log skipping stay exactly as strict.
//!   Files that cannot be read are reported as unsafe evidence instead
//!   of being silently skipped.
//! - Every `#task` row counts, whatever its box holds. Unknown statuses
//!   veto all-closed inference instead of vanishing.
//! - Merge-retired sources (a `merged into` relation edge from
//!   `relations.merge_labels`, or a dated `📥 merged into
//!   [[note#Heading]]` entry under a `❌ **CANCEL LOG**` header) are
//!   followed to their resolved target through the shared relation
//!   resolver. The target's status decides archive evidence. A section
//!   tombstone counts as a closed cancel. Unresolved merges hold the
//!   thread for review. Only configured merge effects count, never prose
//!   substrings.
//! - Saved message IDs are parsed from the validated managed mail-note
//!   cards and frozen with a note fingerprint for later revalidation.
//!   Missing, duplicate, damaged, or mismatched mail notes are refusals,
//!   never zero watchers.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use super::scan::{
    fenced_lines, lower_stem, parse_task_row, scan_note_tasks, DroppedKind,
    DroppedMailLink, LinkPlacement, MailLinks, OPEN_STATUSES,
};
use super::{
    super::{
        note::{index::MailIndex, parse as note_parse},
        vault_walk, GmailError,
    },
    ThreadKey,
};
use crate::native::{
    collect_done::trailing_block_id_in_line,
    config::relations::RelationsConfig,
    done_archive, is_always_excluded_note_directory_name, markdown,
    relation_labels::relation_label_key,
    task_dependencies::blocks as blocks_mod,
    task_relations::{
        parse as relation_parse,
        resolve::{
            resolve_relation_entry, resolve_relation_target_path,
            RelationResolveError, RelationResolveIndex, RelationTargetKind,
        },
        LinksLine, RelationEntry, RelationEntryKind,
    },
    vault_links::{LinkResolution, NoteIndex},
};

/// Statuses that prove a task closed for archive: done or cancelled.
pub(crate) const POSITIVE_CLOSED: [char; 3] = ['x', 'X', '-'];

/// Section-merge provenance marker, `📥 merged into [[note#Heading]]`,
/// matched only inside Cancel Log children, never in prose.
const SECTION_MERGE_MARK: &str = "📥 merged into [[";

/// Follow at most this many merge hops; a longer chain is unresolved.
const MERGE_FOLLOW_DEPTH: u8 = 8;

/// How one linking task's box reads for archive evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArchiveTaskClass {
    /// `[ ]`, `[*]`, `[/]`, `[?]`: still watching, vetoes archiving.
    Open,
    /// `x`, `X`, `-`: positive closed evidence.
    ClosedPositive,
    /// Any other box: unknown, vetoes all-closed inference.
    Unknown,
}

/// Classify one checkbox status for archive evidence.
pub(crate) fn classify_archive_status(status: char) -> ArchiveTaskClass {
    if POSITIVE_CLOSED.contains(&status) {
        ArchiveTaskClass::ClosedPositive
    } else if OPEN_STATUSES.contains(&status) {
        ArchiveTaskClass::Open
    } else {
        ArchiveTaskClass::Unknown
    }
}

/// Why a merge-retired source could not become an effective linker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MergeUnresolvedKind {
    MissingNote,
    MissingBlock,
    AmbiguousBasename,
    NonTask,
    SelfEdge,
    Cycle,
    DepthCap,
}

impl MergeUnresolvedKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::MissingNote => "missing_note",
            Self::MissingBlock => "missing_block",
            Self::AmbiguousBasename => "ambiguous_basename",
            Self::NonTask => "target_not_task",
            Self::SelfEdge => "self_relation",
            Self::Cycle => "cycle",
            Self::DepthCap => "depth_cap",
        }
    }
}

/// One unresolved merge edge that holds its thread for review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MergeUnresolved {
    pub(crate) source_path: String,
    pub(crate) source_line: usize,
    pub(crate) source_block_id: Option<String>,
    /// Raw LINKS target text (`[[note#^id]]`).
    pub(crate) target: String,
    pub(crate) kind: MergeUnresolvedKind,
}

impl MergeUnresolved {
    fn new(
        source: &ArchiveLinkTask,
        target: String,
        kind: MergeUnresolvedKind,
    ) -> Self {
        Self {
            source_path: source.path.clone(),
            source_line: source.line,
            source_block_id: source.block_id.clone(),
            target,
            kind,
        }
    }
}

/// One task row linking a thread's mail note, of any status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveLinkTask {
    /// Vault-relative note path, `/`-separated.
    pub(crate) path: String,
    /// 1-based line number of the task row.
    pub(crate) line: usize,
    /// The task row's exact source text, without its line ending.
    pub(crate) text: String,
    /// The row's trailing `^block-id`, when present.
    pub(crate) block_id: Option<String>,
    /// The checkbox status, of any kind.
    pub(crate) status: char,
    /// What the status proves.
    pub(crate) class: ArchiveTaskClass,
    /// Where the link to the mail note sits.
    pub(crate) placement: LinkPlacement,
    /// Whether the row lives under the top-level `done/` archive.
    pub(crate) in_done: bool,
    /// Whether the row carries merge-retired provenance.
    pub(crate) merge_retired: bool,
    /// Whether this linker entered only by following a merge edge.
    pub(crate) via_merge: bool,
    /// Whether the link resolved only to a duplicated mail-note identity.
    pub(crate) from_duplicate_note: bool,
}

/// Why one thread's mail-note evidence cannot prove completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ArchiveIssue {
    /// The indexed mail note is gone from the vault.
    MissingMailNote { note: String },
    /// The indexed mail note cannot be read.
    UnreadableMailNote { note: String },
    /// The indexed mail note fails strict managed-card parsing.
    DamagedMailNote { note: String },
    /// The mail note's identity does not match the indexed thread.
    IdentityMismatch { note: String },
    /// Several mail notes share this Gmail thread.
    DuplicateMailNote { paths: Vec<String> },
    /// An ambiguous basename link may name this thread's note.
    IdentityUncertain { stem: String },
}

impl ArchiveIssue {
    /// Body-free one-line description for reports and diagnostics.
    pub(crate) fn message(&self) -> String {
        match self {
            ArchiveIssue::MissingMailNote { note } => {
                format!("mail note {note} is indexed but missing")
            }
            ArchiveIssue::UnreadableMailNote { note } => {
                format!("mail note {note} cannot be read")
            }
            ArchiveIssue::DamagedMailNote { note } => {
                format!("mail note {note} has damaged managed cards")
            }
            ArchiveIssue::IdentityMismatch { note } => {
                format!("mail note {note} names a different Gmail thread")
            }
            ArchiveIssue::DuplicateMailNote { paths } => {
                format!(
                    "several mail notes share one Gmail thread: {}",
                    paths.join(", ")
                )
            }
            ArchiveIssue::IdentityUncertain { stem } => {
                format!("an ambiguous {stem} link may name this thread")
            }
        }
    }
}

/// Per-thread archive evidence: every linking task, the frozen saved IDs,
/// and every reason the thread cannot prove completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveThreadEvidence {
    pub(crate) key: ThreadKey,
    /// Canonical indexed mail-note path, `/`-separated.
    pub(crate) note: String,
    /// Every linking task row, sorted by path then line, including
    /// linkers that resolved only to a duplicated mail-note identity.
    pub(crate) tasks: Vec<ArchiveLinkTask>,
    /// Sorted saved message IDs parsed from the managed cards.
    pub(crate) saved_ids: Vec<String>,
    /// `sha256:` hex of the mail-note bytes the IDs were parsed from.
    pub(crate) note_fingerprint: Option<String>,
    /// An unresolved merge edge, after following configured merge labels.
    pub(crate) merge_unresolved: Option<MergeUnresolved>,
    /// An ambiguous basename link may name this thread's note.
    pub(crate) identity_uncertain: bool,
    /// Mail-note refusals: completion is unprovable while non-empty.
    pub(crate) issues: Vec<ArchiveIssue>,
}

impl ArchiveThreadEvidence {
    /// Whether any linking task is still open (live or under `done/`).
    pub(crate) fn has_open(&self) -> bool {
        self.tasks
            .iter()
            .any(|task| task.class == ArchiveTaskClass::Open)
    }

    /// Whether any linking task carries an unknown checkbox status.
    pub(crate) fn has_unknown(&self) -> bool {
        self.tasks
            .iter()
            .any(|task| task.class == ArchiveTaskClass::Unknown)
    }

    /// The positively closed linkers (`x`, `X`, `-`).
    pub(crate) fn positive_closed(&self) -> Vec<&ArchiveLinkTask> {
        self.tasks
            .iter()
            .filter(|task| task.class == ArchiveTaskClass::ClosedPositive)
            .collect()
    }

    /// Whether the mail-note evidence is complete: no refusals, a
    /// fingerprint, and at least one saved ID.
    pub(crate) fn mail_complete(&self) -> bool {
        self.issues.is_empty()
            && self.note_fingerprint.is_some()
            && !self.saved_ids.is_empty()
    }

    /// Whether a merge-retired source could not resolve to a target task.
    pub(crate) fn merge_unresolved(&self) -> bool {
        self.merge_unresolved.is_some()
    }
}

/// The strict archive scan: per-thread evidence plus scan-wide warnings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveScan {
    threads: BTreeMap<ThreadKey, ArchiveThreadEvidence>,
    warnings: Vec<String>,
    incomplete_reads: Vec<String>,
    dropped_ambiguous: Vec<DroppedMailLink>,
}

/// Saved message identities validated from managed mail cards before this
/// sync hydrates any notes. A new card written during the run is deliberately
/// absent, so its message IDs stay unsaved until a later run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SavedIdSnapshot {
    ids_by_thread: BTreeMap<ThreadKey, BTreeSet<String>>,
    complete: bool,
}

impl SavedIdSnapshot {
    pub(crate) fn is_complete(&self) -> bool {
        self.complete
    }
}

impl ArchiveScan {
    /// Small synthetic scan for the archive-state fold tests. Production
    /// scans always use [`scan_archive_evidence`].
    #[cfg(test)]
    pub(crate) fn test_scan(
        evidence: ArchiveThreadEvidence,
        complete: bool,
    ) -> Self {
        let mut incomplete_reads = Vec::new();
        if !complete {
            incomplete_reads.push("tests/unreadable.md".to_string());
        }
        Self {
            threads: BTreeMap::from([(evidence.key.clone(), evidence)]),
            warnings: Vec::new(),
            incomplete_reads,
            dropped_ambiguous: Vec::new(),
        }
    }

    /// Empty scan for state tests that assert missing linkers are not
    /// interpreted as an open or closed transition.
    #[cfg(test)]
    pub(crate) fn test_empty_scan(complete: bool) -> Self {
        Self {
            threads: BTreeMap::new(),
            warnings: Vec::new(),
            incomplete_reads: if complete {
                Vec::new()
            } else {
                vec!["tests/unreadable.md".to_string()]
            },
            dropped_ambiguous: Vec::new(),
        }
    }

    /// One thread's evidence, when any task links its mail note.
    pub(crate) fn evidence(
        &self,
        key: &ThreadKey,
    ) -> Option<&ArchiveThreadEvidence> {
        self.threads.get(key)
    }

    /// Every linked thread, sorted by key.
    pub(crate) fn threads(&self) -> impl Iterator<Item = &ThreadKey> {
        self.threads.keys()
    }

    /// Body-free warnings about links that name no single thread.
    pub(crate) fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Vault-relative paths whose bytes could not be read.
    pub(crate) fn incomplete_reads(&self) -> &[String] {
        &self.incomplete_reads
    }

    /// Ambiguous-basename linkers that name no single thread.
    pub(crate) fn ambiguous_links(&self) -> &[DroppedMailLink] {
        &self.dropped_ambiguous
    }

    /// Whether every vault note was fully read. An incomplete read may
    /// hide a linker for any thread, so nothing is archivable until the
    /// reads succeed.
    pub(crate) fn scan_complete(&self) -> bool {
        self.incomplete_reads.is_empty()
    }

    /// Capture only IDs from complete, validated managed mail cards.
    pub(crate) fn saved_id_snapshot(&self) -> SavedIdSnapshot {
        let ids_by_thread = self
            .threads
            .iter()
            .filter(|(_, evidence)| evidence.mail_complete())
            .map(|(key, evidence)| {
                (key.clone(), evidence.saved_ids.iter().cloned().collect())
            })
            .collect();
        SavedIdSnapshot {
            ids_by_thread,
            complete: self.scan_complete(),
        }
    }

    /// Restrict post-hydration evidence to IDs already present at the start
    /// of this run. Intersecting preserves validation from the current scan
    /// and also drops IDs removed while filing was in progress.
    pub(crate) fn with_saved_id_snapshot(
        &self,
        snapshot: &SavedIdSnapshot,
    ) -> Self {
        let mut scan = self.clone();
        for (key, evidence) in &mut scan.threads {
            let Some(saved_before_run) = snapshot.ids_by_thread.get(key) else {
                evidence.saved_ids.clear();
                continue;
            };
            evidence
                .saved_ids
                .retain(|id| saved_before_run.contains(id));
        }
        scan
    }

    /// Whether one thread's evidence proves all-closed completion: at
    /// least one positively closed linker, no open or unknown linker, no
    /// unresolved merge, no identity doubt, complete mail-note evidence
    /// with saved IDs, and a fully read vault.
    pub(crate) fn archivable(&self, evidence: &ArchiveThreadEvidence) -> bool {
        self.scan_complete()
            && !evidence.tasks.is_empty()
            && !evidence.has_open()
            && !evidence.has_unknown()
            && !evidence.positive_closed().is_empty()
            && !evidence.merge_unresolved()
            && !evidence.identity_uncertain
            && evidence.mail_complete()
    }
}

/// Merge-hold label keys: every configured `relations.merge_labels` key
/// plus the default `merged into` fallback the merge writer uses when no
/// custom label is supplied.
pub(crate) fn merge_hold_keys(relations: &RelationsConfig) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for label in &relations.merge_labels {
        keys.insert(label.key.clone());
    }
    keys.insert(relation_label_key("merged into"));
    keys
}

/// Scan the vault (live notes and `done/`) for archive evidence. Watch
/// placement and identity rules are reused from [`scan`]; only the
/// inclusion set (plus `done/`), the status range, and the strictness
/// around unreadable, ambiguous, and merge-retired evidence differ.
pub(crate) fn scan_archive_evidence(
    vault: &Path,
    mail_folder: &Path,
    index: &MailIndex,
    relations: &RelationsConfig,
) -> Result<ArchiveScan, GmailError> {
    let merge_keys = merge_hold_keys(relations);
    let mut links = MailLinks::new(vault, index);
    let mut notes: Vec<(String, String)> = Vec::new();
    let mut incomplete_reads: Vec<String> = Vec::new();
    visit_archive_notes(vault, mail_folder, &mut notes, &mut incomplete_reads)?;
    notes.sort();

    let resolve_index = ScanRelationIndex::from_notes(&notes, &merge_keys);

    let mut tasks_by_thread: BTreeMap<ThreadKey, Vec<ArchiveLinkTask>> =
        BTreeMap::new();
    let mut dropped_ambiguous: Vec<DroppedMailLink> = Vec::new();
    for (relative, text) in &notes {
        if !text.contains(super::scan::TASK_TAG) || !text.contains("[[") {
            continue;
        }
        let mut dropped: Vec<DroppedMailLink> = Vec::new();
        let found = scan_note_tasks(
            relative,
            text,
            &mut links,
            false,
            Some(&mut dropped),
        );
        if found.is_empty() && dropped.is_empty() {
            continue;
        }
        let in_done = done_archive::is_archive_path(Path::new(relative));
        for task in found {
            let line_retired = resolve_index
                .task_at(&task.path, task.line)
                .is_some_and(|snapshot| snapshot.provenance.is_some());
            for target in &task.targets {
                tasks_by_thread.entry(target.key.clone()).or_default().push(
                    ArchiveLinkTask {
                        path: task.path.clone(),
                        line: task.line,
                        text: task.text.clone(),
                        block_id: task.block_id.clone(),
                        status: task.status,
                        class: classify_archive_status(task.status),
                        placement: target.placement,
                        in_done,
                        merge_retired: line_retired,
                        via_merge: false,
                        from_duplicate_note: false,
                    },
                );
            }
        }
        for link in dropped {
            match link.kind.clone() {
                DroppedKind::Duplicate(key) => {
                    let line_retired = resolve_index
                        .task_at(&link.path, link.line)
                        .is_some_and(|snapshot| snapshot.provenance.is_some());
                    tasks_by_thread.entry(key).or_default().push(
                        ArchiveLinkTask {
                            path: link.path.clone(),
                            line: link.line,
                            text: link.text.clone(),
                            block_id: link.block_id.clone(),
                            status: link.status,
                            class: classify_archive_status(link.status),
                            placement: link.placement,
                            in_done,
                            merge_retired: line_retired,
                            via_merge: false,
                            from_duplicate_note: true,
                        },
                    );
                }
                DroppedKind::Ambiguous => dropped_ambiguous.push(link),
            }
        }
    }

    let ambiguous_stems: BTreeSet<String> = dropped_ambiguous
        .iter()
        .map(|link| link.stem.clone())
        .collect();
    let mut threads: BTreeMap<ThreadKey, ArchiveThreadEvidence> =
        BTreeMap::new();
    for (key, mut linkers) in tasks_by_thread {
        linkers.sort_by(|left, right| {
            left.path.cmp(&right.path).then(left.line.cmp(&right.line))
        });
        let (tasks, merge_unresolved) =
            effective_linkers(linkers, &resolve_index);
        let (note, saved_ids, note_fingerprint, mut issues) =
            mail_note_evidence(vault, index, &key);
        if ambiguous_stems
            .contains(lower_stem(&note).as_deref().unwrap_or_default())
        {
            issues.push(ArchiveIssue::IdentityUncertain {
                stem: lower_stem(&note).unwrap_or_default(),
            });
        }
        threads.insert(
            key.clone(),
            ArchiveThreadEvidence {
                key,
                note,
                tasks,
                saved_ids,
                note_fingerprint,
                merge_unresolved,
                identity_uncertain: issues.iter().any(|issue| {
                    matches!(issue, ArchiveIssue::IdentityUncertain { .. })
                }),
                issues,
            },
        );
    }

    Ok(ArchiveScan {
        threads,
        warnings: links.warnings(),
        incomplete_reads,
        dropped_ambiguous,
    })
}

/// Saved message IDs plus the note fingerprint for one thread, or the
/// refusal that makes completion unprovable. Duplicates refuse before any
/// read: no note file is authoritative for the thread.
fn mail_note_evidence(
    vault: &Path,
    index: &MailIndex,
    key: &ThreadKey,
) -> (String, Vec<String>, Option<String>, Vec<ArchiveIssue>) {
    if let Some((_, _, paths)) =
        index.duplicates.iter().find(|(account, thread_id, _)| {
            ThreadKey::new(account, thread_id).as_ref() == Some(key)
        })
    {
        let mut sorted = paths.clone();
        sorted.sort();
        return (
            sorted.first().cloned().unwrap_or_default(),
            Vec::new(),
            None,
            vec![ArchiveIssue::DuplicateMailNote { paths: sorted }],
        );
    }
    let Some(entry) = index.by_identity.get(&key.index_key()) else {
        return (String::new(), Vec::new(), None, Vec::new());
    };
    let note = entry.path.clone();
    let bytes = match fs::read(vault.join(&note)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (
                note.clone(),
                Vec::new(),
                None,
                vec![ArchiveIssue::MissingMailNote { note }],
            );
        }
        Err(_) => {
            return (
                note.clone(),
                Vec::new(),
                None,
                vec![ArchiveIssue::UnreadableMailNote { note }],
            );
        }
    };
    let fingerprint = format!("sha256:{}", hex::encode(Sha256::digest(&bytes)));
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => {
            return (
                note.clone(),
                Vec::new(),
                None,
                vec![ArchiveIssue::DamagedMailNote { note }],
            );
        }
    };
    let parsed = match note_parse::parse(&text) {
        Ok(parsed) => parsed,
        Err(_) => {
            return (
                note.clone(),
                Vec::new(),
                None,
                vec![ArchiveIssue::DamagedMailNote { note }],
            );
        }
    };
    if !parsed.account.eq_ignore_ascii_case(&entry.account)
        || parsed.thread_id != key.thread_id()
    {
        return (
            note.clone(),
            Vec::new(),
            Some(fingerprint),
            vec![ArchiveIssue::IdentityMismatch { note }],
        );
    }
    let mut saved_ids: Vec<String> =
        parsed.cards.iter().map(|card| card.id.clone()).collect();
    saved_ids.sort();
    (note, saved_ids, Some(fingerprint), Vec::new())
}

/// Merge-retired provenance on one task row.
#[derive(Debug, Clone, PartialEq, Eq)]
enum MergeProvenance {
    Relation(Vec<RelationEntry>),
    Section,
}

/// One vault task the merge follower can land on.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskSnapshot {
    path: String,
    line: usize,
    text: String,
    block_id: Option<String>,
    status: char,
    class: ArchiveTaskClass,
    in_done: bool,
    provenance: Option<MergeProvenance>,
}

/// In-memory relation index over the notes the archive scan already read.
struct ScanRelationIndex {
    notes: NoteIndex,
    archive_paths: BTreeSet<PathBuf>,
    by_line: BTreeMap<(String, usize), TaskSnapshot>,
    by_id: BTreeMap<(PathBuf, String), TaskSnapshot>,
    ids: BTreeSet<(PathBuf, String)>,
}

impl ScanRelationIndex {
    fn from_notes(
        notes: &[(String, String)],
        merge_keys: &BTreeSet<String>,
    ) -> Self {
        let paths: Vec<PathBuf> =
            notes.iter().map(|(path, _)| PathBuf::from(path)).collect();
        let archive_paths: BTreeSet<PathBuf> = paths
            .iter()
            .filter(|path| done_archive::is_archive_path(path))
            .cloned()
            .collect();
        let mut by_line = BTreeMap::new();
        let mut by_id = BTreeMap::new();
        let mut ids = BTreeSet::new();
        for (relative, text) in notes {
            let in_done = done_archive::is_archive_path(Path::new(relative));
            let note_path = PathBuf::from(relative);
            let lines: Vec<&str> = text
                .split_inclusive('\n')
                .map(|line| markdown::split_line_ending(line).0)
                .collect();
            let body_start = markdown::strictly_closed_frontmatter_end(&lines)
                .map_or(0, |end| end + 1);
            let fenced = fenced_lines(&lines, body_start);
            for (index, line) in lines.iter().enumerate() {
                if index < body_start || fenced.contains(&index) {
                    continue;
                }
                if let Some(block_id) = trailing_block_id_in_line(line) {
                    ids.insert((note_path.clone(), block_id));
                }
                let Some(task) = parse_task_row(line) else {
                    continue;
                };
                let snapshot = TaskSnapshot {
                    path: relative.clone(),
                    line: index + 1,
                    text: (*line).to_string(),
                    block_id: trailing_block_id_in_line(line),
                    status: task.status,
                    class: classify_archive_status(task.status),
                    in_done,
                    provenance: line_merge_provenance(
                        &lines, &fenced, index, merge_keys,
                    ),
                };
                if let Some(block_id) = snapshot.block_id.clone() {
                    by_id.insert(
                        (note_path.clone(), block_id),
                        snapshot.clone(),
                    );
                }
                by_line.insert((relative.clone(), index + 1), snapshot);
            }
        }
        Self {
            notes: NoteIndex::from_paths(paths),
            archive_paths,
            by_line,
            by_id,
            ids,
        }
    }

    fn task_at(&self, path: &str, line: usize) -> Option<&TaskSnapshot> {
        self.by_line.get(&(path.to_string(), line))
    }

    fn task_id(&self, path: &Path, block_id: &str) -> Option<&TaskSnapshot> {
        self.by_id.get(&(path.to_path_buf(), block_id.to_string()))
    }
}

impl RelationResolveIndex for ScanRelationIndex {
    fn resolve_note_path(
        &self,
        source_path: &Path,
        target: &str,
    ) -> Option<PathBuf> {
        resolve_relation_target_path(source_path, target, &self.notes, |path| {
            self.archive_paths.contains(path)
        })
    }

    fn target_kind(&self, path: &Path, block_id: &str) -> RelationTargetKind {
        if self
            .by_id
            .contains_key(&(path.to_path_buf(), block_id.to_string()))
        {
            RelationTargetKind::Task
        } else if self
            .ids
            .contains(&(path.to_path_buf(), block_id.to_string()))
        {
            RelationTargetKind::NonTask
        } else {
            RelationTargetKind::Missing
        }
    }
}

/// Replace relation-merged sources with their resolved targets. Section
/// tombstones stay as closed cancels. The first unresolved edge holds the
/// thread.
fn effective_linkers(
    linkers: Vec<ArchiveLinkTask>,
    index: &ScanRelationIndex,
) -> (Vec<ArchiveLinkTask>, Option<MergeUnresolved>) {
    let originals: BTreeMap<(String, usize), ArchiveLinkTask> = linkers
        .iter()
        .map(|task| ((task.path.clone(), task.line), task.clone()))
        .collect();
    let mut result: Vec<ArchiveLinkTask> = Vec::new();
    let mut seen: BTreeSet<(String, usize)> = BTreeSet::new();
    let mut unresolved: Option<MergeUnresolved> = None;

    let mut insert = |task: ArchiveLinkTask| {
        if seen.insert((task.path.clone(), task.line)) {
            result.push(task);
        }
    };

    for linker in &linkers {
        match index
            .task_at(&linker.path, linker.line)
            .and_then(|snapshot| snapshot.provenance.clone())
        {
            Some(MergeProvenance::Section) => {
                insert(section_closed_linker(linker));
            }
            Some(MergeProvenance::Relation(entries)) => {
                for entry in entries {
                    let (targets, error) = follow_merge(
                        linker,
                        &entry,
                        index,
                        &originals,
                        0,
                        &mut Vec::new(),
                    );
                    for target in targets {
                        insert(target);
                    }
                    if unresolved.is_none() {
                        unresolved = error;
                    }
                }
            }
            None => insert(linker.clone()),
        }
    }

    result.sort_by(|left, right| {
        left.path.cmp(&right.path).then(left.line.cmp(&right.line))
    });
    (result, unresolved)
}

fn section_closed_linker(source: &ArchiveLinkTask) -> ArchiveLinkTask {
    let mut task = source.clone();
    task.class = ArchiveTaskClass::ClosedPositive;
    if !POSITIVE_CLOSED.contains(&task.status) {
        task.status = '-';
    }
    task.via_merge = false;
    task
}

fn follow_merge(
    source: &ArchiveLinkTask,
    entry: &RelationEntry,
    index: &ScanRelationIndex,
    originals: &BTreeMap<(String, usize), ArchiveLinkTask>,
    depth: u8,
    stack: &mut Vec<(String, String)>,
) -> (Vec<ArchiveLinkTask>, Option<MergeUnresolved>) {
    if depth >= MERGE_FOLLOW_DEPTH {
        return (
            Vec::new(),
            Some(MergeUnresolved::new(
                source,
                entry.canonical_text(),
                MergeUnresolvedKind::DepthCap,
            )),
        );
    }
    let source_path = Path::new(&source.path);
    match resolve_relation_entry(
        source_path,
        source.block_id.as_deref(),
        entry,
        index,
    ) {
        Ok((path, block_id)) => {
            let identity = (slashes(&path), block_id.clone());
            if stack.iter().any(|seen| seen == &identity) {
                return (
                    Vec::new(),
                    Some(MergeUnresolved::new(
                        source,
                        entry.canonical_text(),
                        MergeUnresolvedKind::Cycle,
                    )),
                );
            }
            let Some(snapshot) = index.task_id(&path, &block_id) else {
                return (
                    Vec::new(),
                    Some(MergeUnresolved::new(
                        source,
                        entry.canonical_text(),
                        MergeUnresolvedKind::MissingBlock,
                    )),
                );
            };
            let task = originals
                .get(&(snapshot.path.clone(), snapshot.line))
                .cloned()
                .unwrap_or_else(|| linker_from_snapshot(snapshot, true));
            follow_resolved_task(
                source, entry, task, snapshot, index, originals, depth, stack,
                identity,
            )
        }
        Err(RelationResolveError::SelfRelation) => (
            Vec::new(),
            Some(MergeUnresolved::new(
                source,
                entry.canonical_text(),
                MergeUnresolvedKind::SelfEdge,
            )),
        ),
        Err(RelationResolveError::TargetNotTask) => (
            Vec::new(),
            Some(MergeUnresolved::new(
                source,
                entry.canonical_text(),
                MergeUnresolvedKind::NonTask,
            )),
        ),
        Err(RelationResolveError::Unresolved) => (
            Vec::new(),
            Some(MergeUnresolved::new(
                source,
                entry.canonical_text(),
                classify_unresolved(source_path, entry, index),
            )),
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn follow_resolved_task(
    source: &ArchiveLinkTask,
    entry: &RelationEntry,
    task: ArchiveLinkTask,
    snapshot: &TaskSnapshot,
    index: &ScanRelationIndex,
    originals: &BTreeMap<(String, usize), ArchiveLinkTask>,
    depth: u8,
    stack: &mut Vec<(String, String)>,
    identity: (String, String),
) -> (Vec<ArchiveLinkTask>, Option<MergeUnresolved>) {
    match &snapshot.provenance {
        Some(MergeProvenance::Relation(entries)) => {
            stack.push(identity);
            let mut terminals = Vec::new();
            let mut unresolved = None;
            for nested in entries {
                let (nested_terminals, nested_error) = follow_merge(
                    &task,
                    nested,
                    index,
                    originals,
                    depth + 1,
                    stack,
                );
                terminals.extend(nested_terminals);
                if unresolved.is_none() {
                    unresolved = nested_error;
                }
            }
            stack.pop();
            if terminals.is_empty() && unresolved.is_none() {
                unresolved = Some(MergeUnresolved::new(
                    source,
                    entry.canonical_text(),
                    MergeUnresolvedKind::MissingBlock,
                ));
            }
            (terminals, unresolved)
        }
        Some(MergeProvenance::Section) => {
            (vec![section_closed_linker(&task)], None)
        }
        None => (vec![task], None),
    }
}

fn linker_from_snapshot(
    snapshot: &TaskSnapshot,
    via_merge: bool,
) -> ArchiveLinkTask {
    ArchiveLinkTask {
        path: snapshot.path.clone(),
        line: snapshot.line,
        text: snapshot.text.clone(),
        block_id: snapshot.block_id.clone(),
        status: snapshot.status,
        class: snapshot.class,
        placement: LinkPlacement::TaskLine,
        in_done: snapshot.in_done,
        merge_retired: snapshot.provenance.is_some(),
        via_merge,
        from_duplicate_note: false,
    }
}

fn classify_unresolved(
    source_path: &Path,
    entry: &RelationEntry,
    index: &ScanRelationIndex,
) -> MergeUnresolvedKind {
    if entry.target.is_empty() {
        return MergeUnresolvedKind::MissingBlock;
    }
    match index
        .notes
        .resolve_detailed(Some(source_path), &entry.target)
    {
        LinkResolution::Ambiguous => MergeUnresolvedKind::AmbiguousBasename,
        LinkResolution::Missing => MergeUnresolvedKind::MissingNote,
        LinkResolution::Found(path) => {
            match index.target_kind(&path, &entry.block_id) {
                RelationTargetKind::NonTask => MergeUnresolvedKind::NonTask,
                RelationTargetKind::Task | RelationTargetKind::Missing => {
                    MergeUnresolvedKind::MissingBlock
                }
            }
        }
    }
}

/// LINKS merge-label entries, else a Cancel Log section tombstone.
fn line_merge_provenance(
    lines: &[&str],
    fenced: &BTreeSet<usize>,
    task_index: usize,
    merge_keys: &BTreeSet<String>,
) -> Option<MergeProvenance> {
    lines.get(task_index)?;
    let mut relation_entries = Vec::new();
    for child in relation_parse::links_children_of(lines, fenced, task_index) {
        let LinksLine::Accepted { entries, .. } = &child.parsed else {
            continue;
        };
        for entry in entries {
            if entry.kind != RelationEntryKind::Relation {
                continue;
            }
            if entry.label.as_ref().is_some_and(|label| {
                merge_keys.contains(&relation_label_key(label))
            }) {
                relation_entries.push(entry.clone());
            }
        }
    }
    if !relation_entries.is_empty() {
        return Some(MergeProvenance::Relation(relation_entries));
    }
    for header in blocks_mod::direct_child_lines(lines, fenced, task_index) {
        if !lines
            .get(header)
            .is_some_and(|line| blocks_mod::is_cancel_log(line))
        {
            continue;
        }
        for entry in blocks_mod::direct_child_lines(lines, fenced, header) {
            if lines
                .get(entry)
                .is_some_and(|line| line.contains(SECTION_MERGE_MARK))
            {
                return Some(MergeProvenance::Section);
            }
        }
    }
    None
}

/// Every Markdown note the archive scan reads: like
/// [`vault_walk::scan_vault_notes`] but including the top-level `done/`
/// archive. Files that cannot be read as UTF-8 are reported in
/// `incomplete` instead of being silently skipped.
fn visit_archive_notes(
    vault: &Path,
    mail_folder: &Path,
    notes: &mut Vec<(String, String)>,
    incomplete: &mut Vec<String>,
) -> Result<(), GmailError> {
    let canonical_vault = vault.canonicalize().map_err(|_| {
        GmailError::runtime("note", "the vault path is unavailable".to_string())
    })?;
    walk_archive_notes(
        &canonical_vault,
        &canonical_vault,
        mail_folder,
        notes,
        incomplete,
    )
}

fn walk_archive_notes(
    root: &Path,
    directory: &Path,
    mail_folder: &Path,
    notes: &mut Vec<(String, String)>,
    incomplete: &mut Vec<String>,
) -> Result<(), GmailError> {
    let entries = fs::read_dir(directory).map_err(|error| {
        GmailError::runtime(
            "note",
            format!("could not scan the vault: {error}"),
        )
    })?;
    let mut entries =
        entries.collect::<Result<Vec<_>, _>>().map_err(|error| {
            GmailError::runtime(
                "note",
                format!("could not scan the vault: {error}"),
            )
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let kind = entry.file_type().map_err(|error| {
            GmailError::runtime(
                "note",
                format!("could not inspect a vault path: {error}"),
            )
        })?;
        if kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        if kind.is_dir() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.')
                || is_always_excluded_note_directory_name(&name)
                || vault_walk::path_in_folder(relative, mail_folder)
            {
                continue;
            }
            walk_archive_notes(root, &path, mail_folder, notes, incomplete)?;
        } else if kind.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            if vault_walk::path_in_folder(relative, mail_folder) {
                continue;
            }
            let relative = slashes(relative);
            match fs::read(&path) {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(text) => {
                        if vault_walk::is_zorg_generated(&text) {
                            continue;
                        }
                        notes.push((relative, text));
                    }
                    Err(_) => incomplete.push(relative),
                },
                Err(_) => incomplete.push(relative),
            }
        }
    }
    Ok(())
}

fn slashes(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::native::config::relations::CancelLabel;
    use crate::native::gmail::note::index::MailIndexEntry;
    use crate::native::gmail::sync::scan::scan_watching_tasks;

    const ACCOUNT: &str = "me@example.com";
    const THREAD: &str = "aabbccddeeff0011";
    const OTHER: &str = "bb02bb02bb02bb02";
    const THIRD: &str = "cc03cc03cc03cc03";

    struct Vault {
        _temp: tempfile::TempDir,
        root: PathBuf,
        index: MailIndex,
    }

    impl Vault {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().to_path_buf();
            Self {
                _temp: temp,
                root,
                index: MailIndex::default(),
            }
        }

        fn write(&self, relative: &str, text: &str) {
            let path = self.root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        /// Write a valid managed mail note and index it.
        fn mail(
            &mut self,
            relative: &str,
            account: &str,
            thread: &str,
            ids: &[&str],
        ) {
            self.write(relative, &mail_note_text(account, thread, ids));
            self.index.by_identity.insert(
                (account.to_ascii_lowercase(), thread.to_ascii_lowercase()),
                MailIndexEntry {
                    account: account.to_string(),
                    thread_id: thread.to_ascii_lowercase(),
                    path: relative.to_string(),
                    title: "Lease".to_string(),
                    who: "Dana".to_string(),
                    label: "✉️ Lease · Dana".to_string(),
                    started: String::new(),
                    updated: String::new(),
                    messages: ids.len() as u64,
                    turn: None,
                    turn_since: None,
                },
            );
        }

        fn scan(&self) -> ArchiveScan {
            self.scan_with(&RelationsConfig::default())
        }

        fn scan_with(&self, relations: &RelationsConfig) -> ArchiveScan {
            scan_archive_evidence(
                &self.root,
                Path::new("mail"),
                &self.index,
                relations,
            )
            .unwrap()
        }
    }

    fn key(thread: &str) -> ThreadKey {
        ThreadKey::new(ACCOUNT, thread).unwrap()
    }

    fn evidence<'scan>(
        scan: &'scan ArchiveScan,
        thread: &str,
    ) -> &'scan ArchiveThreadEvidence {
        scan.evidence(&key(thread)).unwrap()
    }

    fn mail_note_text(account: &str, thread: &str, ids: &[&str]) -> String {
        let mut cards = String::new();
        for id in ids {
            cards.push_str(&format!(
                "<!-- bob:gmail:msg {id} - -->\n\
                 > [!mail+] **Dana** · Fri at 9:46 AM to you\n\
                 > body\n\
                 \n\
                 ^m-{id}\n\
                 \n"
            ));
        }
        format!(
            "---\n\
             parent: \"[[mail]]\"\n\
             type: \"[[mail]]\"\n\
             content: untrusted-email\n\
             gmail_account: {account}\n\
             gmail_thread_id: {thread}\n\
             title: Lease\n\
             who: Dana\n\
             started: 2026-09-19\n\
             updated: 2026-09-19\n\
             messages: {count}\n\
             ---\n\
             \n\
             # Lease\n\
             \n\
             [Open in Gmail](https://mail.google.com/)\n\
             · Dana ↔ you · {count} messages · Sep 19 → Sep 19\n\
             \n\
             ## Notes\n\
             \n\
             ## Conversation\n\
             \n\
             <!-- bob:gmail:begin v1 -->\n\
             {cards}\
             <!-- bob:gmail:end -->\n",
            count = ids.len(),
        )
    }

    fn relations_with_merge(names: &[&str]) -> RelationsConfig {
        RelationsConfig {
            merge_labels: names
                .iter()
                .map(|name| CancelLabel {
                    key: relation_label_key(name),
                    name: (*name).to_string(),
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn status_classes_split_open_closed_and_unknown() {
        assert_eq!(classify_archive_status(' '), ArchiveTaskClass::Open);
        assert_eq!(classify_archive_status('*'), ArchiveTaskClass::Open);
        assert_eq!(classify_archive_status('/'), ArchiveTaskClass::Open);
        assert_eq!(classify_archive_status('?'), ArchiveTaskClass::Open);
        assert_eq!(
            classify_archive_status('x'),
            ArchiveTaskClass::ClosedPositive
        );
        assert_eq!(
            classify_archive_status('X'),
            ArchiveTaskClass::ClosedPositive
        );
        assert_eq!(
            classify_archive_status('-'),
            ArchiveTaskClass::ClosedPositive
        );
        assert_eq!(classify_archive_status('>'), ArchiveTaskClass::Unknown);
    }

    #[test]
    fn positive_closed_covers_cancelled_uppercase_and_done() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, THREAD, &[THREAD]);
        vault.write("job.md", "- [X] #task Done [[mail/2026/lease]]\n");
        vault.write(
            "done/old.md",
            "- [-] #task Cancelled [[mail/2026/lease]] ^old\n",
        );
        let scan = vault.scan();
        assert!(scan.scan_complete());
        assert!(scan.incomplete_reads().is_empty());
        assert!(scan.warnings().is_empty());
        let found = evidence(&scan, THREAD);
        assert_eq!(found.note, "mail/2026/lease.md");
        assert_eq!(found.saved_ids, vec![THREAD.to_string()]);
        assert!(found
            .note_fingerprint
            .as_deref()
            .is_some_and(|print| print.starts_with("sha256:")));
        assert_eq!(found.positive_closed().len(), 2);
        assert!(!found.has_open());
        assert!(!found.has_unknown());
        assert!(!found.merge_unresolved());
        assert!(!found.identity_uncertain);
        assert!(found.issues.is_empty());
        assert!(found.mail_complete());
        assert!(scan.archivable(found));
        let done_row = found
            .tasks
            .iter()
            .find(|task| task.path == "done/old.md")
            .unwrap();
        assert!(done_row.in_done);
        assert_eq!(done_row.status, '-');
        assert_eq!(done_row.block_id.as_deref(), Some("old"));
        assert_eq!(found.tasks[0].path, "done/old.md");
        assert_eq!(found.tasks[1].path, "job.md");
    }

    #[test]
    fn open_linkers_veto_including_open_rows_in_done() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, THREAD, &[THREAD]);
        vault.mail("mail/2026/quote.md", ACCOUNT, OTHER, &[OTHER]);
        vault.write(
            "job.md",
            "- [x] #task Done [[mail/2026/lease]]\n\
             - [ ] #task Still open [[mail/2026/lease]]\n",
        );
        // An open row accidentally residing in done/ still vetoes.
        vault.write(
            "done/late.md",
            "- [x] #task Done [[mail/2026/quote]]\n\
             - [*] #task Moved but open [[mail/2026/quote]]\n",
        );
        let scan = vault.scan();
        let leased = evidence(&scan, THREAD);
        assert_eq!(leased.positive_closed().len(), 1);
        assert!(leased.has_open());
        assert!(!scan.archivable(leased));
        let quoted = evidence(&scan, OTHER);
        assert_eq!(quoted.positive_closed().len(), 1);
        assert!(quoted.has_open());
        assert!(!scan.archivable(quoted));
    }

    #[test]
    fn no_linkers_means_no_evidence() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, THREAD, &[THREAD]);
        vault.write(
            "job.md",
            "- [x] #task Done without a link\n\
             - [ ] #task Open without a link\n",
        );
        let scan = vault.scan();
        assert!(scan.evidence(&key(THREAD)).is_none());
        assert_eq!(scan.threads().count(), 0);
        assert!(scan.scan_complete());
    }

    #[test]
    fn unknown_statuses_veto_all_closed() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, THREAD, &[THREAD]);
        vault.write(
            "job.md",
            "- [x] #task Done [[mail/2026/lease]]\n\
             - [>] #task Deferred custom [[mail/2026/lease]]\n",
        );
        let scan = vault.scan();
        let found = evidence(&scan, THREAD);
        assert_eq!(found.positive_closed().len(), 1);
        assert!(!found.has_open());
        assert!(found.has_unknown());
        assert!(!scan.archivable(found));
    }

    #[test]
    fn multiple_threads_attribute_independently() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, THREAD, &[THREAD]);
        vault.mail("mail/2026/quote.md", ACCOUNT, OTHER, &[OTHER]);
        vault.write(
            "job.md",
            "- [x] #task Both [[mail/2026/lease]] and [[mail/2026/quote]]\n",
        );
        vault.write("cash.md", "- [-] #task Pay [[mail/2026/lease]]\n");
        let scan = vault.scan();
        let leased = evidence(&scan, THREAD);
        assert_eq!(leased.tasks.len(), 2);
        assert!(scan.archivable(leased));
        let quoted = evidence(&scan, OTHER);
        assert_eq!(quoted.tasks.len(), 1);
        assert_eq!(quoted.tasks[0].path, "job.md");
        assert!(scan.archivable(quoted));
    }

    #[test]
    fn placement_and_exclusion_rules_match_watch() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, THREAD, &[THREAD]);
        let row = "- [x] #task Watch [[mail/2026/lease]]\n";
        vault.write(
            "kept.md",
            "- [x] #task Own line [[mail/2026/lease]]\n\
             - [x] #task Child link\n\
             \t- [[mail/2026/lease]]\n\
             - [x] #task Prose child\n\
             \t- read [[mail/2026/lease]] first\n\
             - [x] #task Deeper descendant\n\
             \t- context\n\
             \t\t- [[mail/2026/lease]]\n\
             - [x] #task Embed only\n\
             \t- ![[mail/2026/lease]]\n\
             - [x] #task sibling after the block\n\
             - [[mail/2026/lease]]\n",
        );
        vault.write(
            "code.md",
            "```markdown\n\
             - [x] #task Example [[mail/2026/lease]]\n\
             ```\n",
        );
        vault.write(
            "front.md",
            "---\nlink: \"- [x] #task [[mail/2026/lease]]\"\n---\n",
        );
        for excluded in [
            "mail/2026/inside.md",
            "done_ok_but_excluded/_generated/report.md",
            "_conflicts/job.md",
            ".obsidian/x.md",
            ".bob/gmail/x.md",
            ".hidden/x.md",
        ] {
            vault.write(excluded, row);
        }
        vault.write("_generated/report.md", row);
        vault.write(
            "log.md",
            &format!("---\ngenerated_from_zorg: true\n---\n{row}"),
        );
        vault.write("done/old.md", row);
        let scan = vault.scan();
        let found = evidence(&scan, THREAD);
        let mut seen: Vec<(String, usize)> = found
            .tasks
            .iter()
            .map(|task| (task.path.clone(), task.line))
            .collect();
        seen.sort();
        assert_eq!(
            seen,
            vec![
                ("done/old.md".to_string(), 1),
                ("kept.md".to_string(), 1),
                ("kept.md".to_string(), 2),
            ]
        );
        let child = found.tasks.iter().find(|task| task.line == 2).unwrap();
        assert_eq!(child.placement, LinkPlacement::ChildBullet { line: 3 });
        assert!(scan.archivable(found));
    }

    #[test]
    fn watch_scan_still_excludes_done() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, THREAD, &[THREAD]);
        vault.write("done/old.md", "- [ ] #task Moved [[mail/2026/lease]]\n");
        let watch =
            scan_watching_tasks(&vault.root, Path::new("mail"), &vault.index)
                .unwrap();
        assert!(!watch.is_watched(&key(THREAD)));
        let scan = vault.scan();
        let found = evidence(&scan, THREAD);
        assert_eq!(found.tasks.len(), 1);
        assert!(found.tasks[0].in_done);
        assert!(found.has_open());
    }

    #[test]
    fn renamed_notes_resolve_by_path_then_unique_basename() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/renamed.md", ACCOUNT, THREAD, &[THREAD]);
        vault.write(
            "job.md",
            "- [x] #task Basename [[renamed|label]]\n\
             - [x] #task Exact [[mail/2026/renamed]]\n",
        );
        let scan = vault.scan();
        let found = evidence(&scan, THREAD);
        assert_eq!(found.tasks.len(), 2);
        assert!(scan.archivable(found));
    }

    #[test]
    fn ambiguous_basenames_hold_affected_threads() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/notes.md", ACCOUNT, THREAD, &[THREAD]);
        vault.write("projects/notes.md", "plain\n");
        vault.write(
            "job.md",
            "- [x] #task Exact [[mail/2026/notes]]\n\
             - [ ] #task Vague [[notes]]\n",
        );
        let scan = vault.scan();
        assert_eq!(scan.ambiguous_links().len(), 1);
        assert_eq!(scan.ambiguous_links()[0].stem, "notes");
        assert_eq!(scan.ambiguous_links()[0].line, 2);
        assert!(!scan.warnings().is_empty());
        let found = evidence(&scan, THREAD);
        assert_eq!(found.tasks.len(), 1);
        assert!(found.identity_uncertain);
        assert_eq!(
            found.issues,
            vec![ArchiveIssue::IdentityUncertain {
                stem: "notes".to_string(),
            }]
        );
        assert!(!scan.archivable(found));
    }

    #[test]
    fn duplicate_identities_are_refusals_with_linkers_kept() {
        let mut vault = Vault::new();
        vault.write(
            "mail/2026/dup-a.md",
            &mail_note_text(ACCOUNT, THREAD, &[THREAD]),
        );
        vault.write(
            "mail/2026/dup-b.md",
            &mail_note_text(ACCOUNT, THREAD, &[THREAD]),
        );
        vault.index.duplicates.push((
            ACCOUNT.to_string(),
            THREAD.to_string(),
            vec![
                "mail/2026/dup-a.md".to_string(),
                "mail/2026/dup-b.md".to_string(),
            ],
        ));
        vault.write("job.md", "- [x] #task Done [[mail/2026/dup-a]]\n");
        let scan = vault.scan();
        let found = evidence(&scan, THREAD);
        // A refusal, not zero watchers: the linker is kept.
        assert_eq!(found.tasks.len(), 1);
        assert_eq!(found.tasks[0].status, 'x');
        assert!(found.tasks[0].from_duplicate_note);
        assert!(found.saved_ids.is_empty());
        assert_eq!(
            found.issues,
            vec![ArchiveIssue::DuplicateMailNote {
                paths: vec![
                    "mail/2026/dup-a.md".to_string(),
                    "mail/2026/dup-b.md".to_string(),
                ],
            }]
        );
        assert!(!found.mail_complete());
        assert!(!scan.archivable(found));
        assert!(scan
            .warnings()
            .iter()
            .any(|warning| warning.contains("share one Gmail thread")));
    }

    #[test]
    fn missing_damaged_and_mismatched_mail_notes_are_refusals() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/quote.md", ACCOUNT, OTHER, &[OTHER]);
        // Missing: indexed but never written.
        vault.index.by_identity.insert(
            (ACCOUNT.to_string(), THIRD.to_string()),
            MailIndexEntry {
                account: ACCOUNT.to_string(),
                thread_id: THIRD.to_string(),
                path: "mail/2026/gone.md".to_string(),
                title: "Gone".to_string(),
                who: "Dana".to_string(),
                label: "gone".to_string(),
                started: String::new(),
                updated: String::new(),
                messages: 1,
                turn: None,
                turn_since: None,
            },
        );
        vault.write("mail/2026/broken.md", "not a mail note\n");
        vault.index.by_identity.insert(
            (ACCOUNT.to_string(), "dd04dd04dd04dd04".to_string()),
            MailIndexEntry {
                account: ACCOUNT.to_string(),
                thread_id: "dd04dd04dd04dd04".to_string(),
                path: "mail/2026/broken.md".to_string(),
                title: "Broken".to_string(),
                who: "Dana".to_string(),
                label: "broken".to_string(),
                started: String::new(),
                updated: String::new(),
                messages: 1,
                turn: None,
                turn_since: None,
            },
        );
        // Mismatched: the file names another thread.
        vault.write(
            "mail/2026/mismatch.md",
            &mail_note_text(ACCOUNT, OTHER, &[OTHER]),
        );
        vault.index.by_identity.insert(
            (ACCOUNT.to_string(), "ee05ee05ee05ee05".to_string()),
            MailIndexEntry {
                account: ACCOUNT.to_string(),
                thread_id: "ee05ee05ee05ee05".to_string(),
                path: "mail/2026/mismatch.md".to_string(),
                title: "Mismatch".to_string(),
                who: "Dana".to_string(),
                label: "mismatch".to_string(),
                started: String::new(),
                updated: String::new(),
                messages: 1,
                turn: None,
                turn_since: None,
            },
        );
        vault.write(
            "job.md",
            "- [x] #task Gone [[mail/2026/gone]]\n\
             - [x] #task Broken [[mail/2026/broken]]\n\
             - [x] #task Mismatch [[mail/2026/mismatch]]\n\
             - [x] #task Fine [[mail/2026/quote]]\n",
        );
        let scan = vault.scan();
        let gone = scan
            .evidence(&ThreadKey::new(ACCOUNT, THIRD).unwrap())
            .unwrap();
        assert_eq!(
            gone.issues,
            vec![ArchiveIssue::MissingMailNote {
                note: "mail/2026/gone.md".to_string(),
            }]
        );
        assert!(!scan.archivable(gone));
        let broken = scan
            .evidence(&ThreadKey::new(ACCOUNT, "dd04dd04dd04dd04").unwrap())
            .unwrap();
        assert_eq!(
            broken.issues,
            vec![ArchiveIssue::DamagedMailNote {
                note: "mail/2026/broken.md".to_string(),
            }]
        );
        assert!(!scan.archivable(broken));
        let mismatch = scan
            .evidence(&ThreadKey::new(ACCOUNT, "ee05ee05ee05ee05").unwrap())
            .unwrap();
        assert_eq!(
            mismatch.issues,
            vec![ArchiveIssue::IdentityMismatch {
                note: "mail/2026/mismatch.md".to_string(),
            }]
        );
        assert!(mismatch.note_fingerprint.is_some());
        assert!(!scan.archivable(mismatch));
        let fine = evidence(&scan, OTHER);
        assert!(scan.archivable(fine));
    }

    #[test]
    fn configured_merge_labels_follow_a_closed_target() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, THREAD, &[THREAD]);
        vault.mail("mail/2026/quote.md", ACCOUNT, OTHER, &[OTHER]);
        vault.write(
            "job.md",
            "- [-] #task Retired [[mail/2026/lease]] ^src\n\
             \t- 🪢 **LINKS:** My Merge [[job#^dst]]\n\
             - [x] #task Target [[mail/2026/lease]] ^dst\n\
             - [x] #task Plain [[mail/2026/quote]] ^plain\n\
             \t- 🪢 **LINKS:** [[other#^y]]\n",
        );
        let relations = relations_with_merge(&["My Merge"]);
        let scan = vault.scan_with(&relations);
        let followed = evidence(&scan, THREAD);
        assert!(!followed.merge_unresolved());
        assert!(!followed.has_open());
        assert_eq!(followed.positive_closed().len(), 1);
        assert_eq!(followed.tasks[0].block_id.as_deref(), Some("dst"));
        assert!(scan.archivable(followed));
        let plain = evidence(&scan, OTHER);
        assert!(!plain.merge_unresolved());
        assert!(!plain.tasks[0].merge_retired);
        assert!(scan.archivable(plain));
        // Without the configured label, the custom edge is not provenance
        // and the retired source stays an effective closed linker.
        let unconfigured = vault.scan();
        let raw = evidence(&unconfigured, THREAD);
        assert!(!raw.merge_unresolved());
        assert_eq!(raw.tasks.len(), 2);
        assert!(unconfigured.archivable(raw));
    }

    #[test]
    fn missing_merge_target_is_unresolved_and_section_tombstones_close() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, THREAD, &[THREAD]);
        vault.mail("mail/2026/quote.md", ACCOUNT, OTHER, &[OTHER]);
        vault.mail("mail/2026/kick.md", ACCOUNT, THIRD, &[THIRD]);
        vault.write(
            "job.md",
            "- [-] #task Retired [[mail/2026/lease]] ^src\n\
             \t- 🪢 **LINKS:** merged into [[gone#^nope]]\n",
        );
        vault.write(
            "section.md",
            "- [-] #task Folded [[mail/2026/quote]] ^fold\n\
             \t- ❌ **CANCEL LOG**\n\
             \t\t- *2026-11-03* — 📥 merged into [[areas/work#Future Work]]\n",
        );
        vault.write(
            "prose.md",
            "- [-] #task Prose [[mail/2026/kick]] ^prose\n\
             \t- prose 📥 merged into [[areas/work#Future Work]]\n\
             - [-] #task Fenced [[mail/2026/kick]] ^fenced\n\
             \t- ❌ **CANCEL LOG**\n\
             \t\t```text\n\
             \t\t- *2026-11-03* — 📥 merged into [[areas/work#Future Work]]\n\
             \t\t```\n",
        );
        let scan = vault.scan();
        let fallback = evidence(&scan, THREAD);
        assert!(fallback.merge_unresolved());
        assert_eq!(
            fallback.merge_unresolved.as_ref().map(|hold| hold.kind),
            Some(MergeUnresolvedKind::MissingNote)
        );
        assert!(!scan.archivable(fallback));
        let section = evidence(&scan, OTHER);
        assert!(!section.merge_unresolved());
        assert_eq!(section.positive_closed().len(), 1);
        assert!(scan.archivable(section));
        let prose = evidence(&scan, THIRD);
        assert_eq!(prose.tasks.len(), 2);
        assert!(!prose.merge_unresolved());
        assert!(scan.archivable(prose));
    }

    #[test]
    fn closed_via_merge_target_archives_without_linking_the_mail() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, THREAD, &[THREAD]);
        vault.write(
            "job.md",
            "- [-] #task Retired [[mail/2026/lease]] ^src\n\
             \t- 🪢 **LINKS:** merged into [[job#^dst]]\n\
             - [x] #task Target without a mail link ^dst\n",
        );
        let scan = vault.scan();
        let found = evidence(&scan, THREAD);
        assert!(!found.merge_unresolved());
        assert_eq!(found.tasks.len(), 1);
        assert!(found.tasks[0].via_merge);
        assert_eq!(found.tasks[0].block_id.as_deref(), Some("dst"));
        assert!(scan.archivable(found));
    }

    #[test]
    fn open_merge_target_is_watching() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, THREAD, &[THREAD]);
        vault.write(
            "job.md",
            "- [-] #task Retired [[mail/2026/lease]] ^src\n\
             \t- 🪢 **LINKS:** merged into [[job#^dst]]\n\
             - [ ] #task Open target ^dst\n",
        );
        let scan = vault.scan();
        let found = evidence(&scan, THREAD);
        assert!(!found.merge_unresolved());
        assert!(found.has_open());
        assert!(!scan.archivable(found));
    }

    #[test]
    fn done_merge_target_archives() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, THREAD, &[THREAD]);
        vault.write(
            "job.md",
            "- [-] #task Retired [[mail/2026/lease]] ^src\n\
             \t- 🪢 **LINKS:** merged into [[old#^dst]]\n",
        );
        vault.write(
            "done/old.md",
            "- [x] #task Archived target [[mail/2026/lease]] ^dst\n",
        );
        let scan = vault.scan();
        let found = evidence(&scan, THREAD);
        assert!(!found.merge_unresolved());
        assert!(found.tasks.iter().any(|task| task.in_done));
        assert!(scan.archivable(found));
    }

    #[test]
    fn merge_chain_follows_to_the_final_target() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, THREAD, &[THREAD]);
        vault.write(
            "job.md",
            "- [-] #task Source [[mail/2026/lease]] ^src\n\
             \t- 🪢 **LINKS:** merged into [[job#^mid]]\n\
             - [-] #task Mid ^mid\n\
             \t- 🪢 **LINKS:** merged into [[job#^dst]]\n\
             - [x] #task Final [[mail/2026/lease]] ^dst\n",
        );
        let scan = vault.scan();
        let found = evidence(&scan, THREAD);
        assert!(!found.merge_unresolved());
        assert_eq!(found.tasks.len(), 1);
        assert_eq!(found.tasks[0].block_id.as_deref(), Some("dst"));
        assert!(scan.archivable(found));
    }

    fn block_ids(found: &ArchiveThreadEvidence) -> Vec<&str> {
        found
            .tasks
            .iter()
            .map(|task| task.block_id.as_deref().unwrap_or(""))
            .collect()
    }

    #[test]
    fn branching_merge_keeps_open_and_closed_terminals_in_either_order() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, THREAD, &[THREAD]);
        vault.mail("mail/2026/quote.md", ACCOUNT, OTHER, &[OTHER]);
        vault.write(
            "job.md",
            "- [-] #task Source [[mail/2026/lease]] ^src\n\
             \t- 🪢 **LINKS:** merged into [[job#^mid]]\n\
             - [-] #task Mid ^mid\n\
             \t- 🪢 **LINKS:** merged into [[job#^open]] • merged into [[job#^closed]]\n\
             - [ ] #task Open ^open\n\
             - [x] #task Closed ^closed\n\
             - [-] #task Reverse [[mail/2026/quote]] ^rev\n\
             \t- 🪢 **LINKS:** merged into [[job#^mid-rev]]\n\
             - [-] #task Mid reverse ^mid-rev\n\
             \t- 🪢 **LINKS:** merged into [[job#^closed-rev]] • merged into [[job#^open-rev]]\n\
             - [x] #task Closed reverse ^closed-rev\n\
             - [ ] #task Open reverse ^open-rev\n",
        );
        let scan = vault.scan();
        let open_first = evidence(&scan, THREAD);
        assert!(!open_first.merge_unresolved());
        assert!(open_first.has_open());
        assert_eq!(block_ids(open_first), ["open", "closed"]);
        assert!(!scan.archivable(open_first));
        let closed_first = evidence(&scan, OTHER);
        assert!(!closed_first.merge_unresolved());
        assert!(closed_first.has_open());
        assert_eq!(block_ids(closed_first), ["closed-rev", "open-rev"]);
        assert!(!scan.archivable(closed_first));
    }

    #[test]
    fn branching_merge_keeps_two_closed_terminals_and_dedupes_diamonds() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, THREAD, &[THREAD]);
        vault.mail("mail/2026/quote.md", ACCOUNT, OTHER, &[OTHER]);
        vault.write(
            "job.md",
            "- [-] #task Source [[mail/2026/lease]] ^src\n\
             \t- 🪢 **LINKS:** merged into [[job#^mid]]\n\
             - [-] #task Mid ^mid\n\
             \t- 🪢 **LINKS:** merged into [[job#^left]] • merged into [[job#^right]]\n\
             - [x] #task Left [completion:: 2026-11-01] ^left\n\
             - [x] #task Right [completion:: 2026-11-04] ^right\n\
             - [-] #task Diamond [[mail/2026/quote]] ^d-src\n\
             \t- 🪢 **LINKS:** merged into [[job#^a]] • merged into [[job#^b]]\n\
             - [-] #task A ^a\n\
             \t- 🪢 **LINKS:** merged into [[job#^shared]]\n\
             - [-] #task B ^b\n\
             \t- 🪢 **LINKS:** merged into [[job#^shared]]\n\
             - [x] #task Shared ^shared\n",
        );
        let scan = vault.scan();
        let branched = evidence(&scan, THREAD);
        assert!(!branched.merge_unresolved());
        assert_eq!(block_ids(branched), ["left", "right"]);
        assert!(scan.archivable(branched));
        let newest =
            super::super::reopen_plan::select_newest_linker(branched).unwrap();
        assert_eq!(newest.block_id.as_deref(), Some("right"));
        let diamond = evidence(&scan, OTHER);
        assert!(!diamond.merge_unresolved());
        assert_eq!(block_ids(diamond), ["shared"]);
        assert!(diamond.tasks[0].via_merge);
        assert!(scan.archivable(diamond));
    }

    #[test]
    fn branching_merge_holds_when_one_branch_is_unresolved() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, THREAD, &[THREAD]);
        vault.mail("mail/2026/quote.md", ACCOUNT, OTHER, &[OTHER]);
        vault.write(
            "job.md",
            "- [-] #task Missing [[mail/2026/lease]] ^src\n\
             \t- 🪢 **LINKS:** merged into [[job#^mid]]\n\
             - [-] #task Mid ^mid\n\
             \t- 🪢 **LINKS:** merged into [[job#^open]] • merged into [[gone#^nope]]\n\
             - [ ] #task Open ^open\n\
             - [-] #task Cycle [[mail/2026/quote]] ^cyc\n\
             \t- 🪢 **LINKS:** merged into [[job#^mid-cyc]]\n\
             - [-] #task Mid cycle ^mid-cyc\n\
             \t- 🪢 **LINKS:** merged into [[job#^ok]] • merged into [[job#^loop]]\n\
             - [x] #task Ok ^ok\n\
             - [-] #task Loop ^loop\n\
             \t- 🪢 **LINKS:** merged into [[job#^mid-cyc]]\n",
        );
        let scan = vault.scan();
        let missing = evidence(&scan, THREAD);
        assert_eq!(
            missing.merge_unresolved.as_ref().map(|hold| hold.kind),
            Some(MergeUnresolvedKind::MissingNote)
        );
        assert_eq!(block_ids(missing), ["open"]);
        assert!(missing.has_open());
        assert!(!scan.archivable(missing));
        let cyclic = evidence(&scan, OTHER);
        assert_eq!(
            cyclic.merge_unresolved.as_ref().map(|hold| hold.kind),
            Some(MergeUnresolvedKind::Cycle)
        );
        assert_eq!(block_ids(cyclic), ["ok"]);
        assert!(!scan.archivable(cyclic));
    }

    #[test]
    fn merge_cycle_is_unresolved() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, THREAD, &[THREAD]);
        vault.write(
            "job.md",
            "- [-] #task Left [[mail/2026/lease]] ^left\n\
             \t- 🪢 **LINKS:** merged into [[job#^right]]\n\
             - [-] #task Right ^right\n\
             \t- 🪢 **LINKS:** merged into [[job#^left]]\n",
        );
        let scan = vault.scan();
        let found = evidence(&scan, THREAD);
        assert_eq!(
            found.merge_unresolved.as_ref().map(|hold| hold.kind),
            Some(MergeUnresolvedKind::Cycle)
        );
        assert!(!scan.archivable(found));
    }

    #[test]
    fn merge_self_edge_and_non_task_are_unresolved() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, THREAD, &[THREAD]);
        vault.mail("mail/2026/quote.md", ACCOUNT, OTHER, &[OTHER]);
        vault.write(
            "job.md",
            "- [-] #task Self [[mail/2026/lease]] ^src\n\
             \t- 🪢 **LINKS:** merged into [[job#^src]]\n\
             - [-] #task Non task [[mail/2026/quote]] ^from\n\
             \t- 🪢 **LINKS:** merged into [[job#^note]]\n\
             A paragraph ^note\n",
        );
        let scan = vault.scan();
        let self_edge = evidence(&scan, THREAD);
        assert_eq!(
            self_edge.merge_unresolved.as_ref().map(|hold| hold.kind),
            Some(MergeUnresolvedKind::SelfEdge)
        );
        let non_task = evidence(&scan, OTHER);
        assert_eq!(
            non_task.merge_unresolved.as_ref().map(|hold| hold.kind),
            Some(MergeUnresolvedKind::NonTask)
        );
    }

    #[test]
    fn merge_depth_cap_is_unresolved_and_eight_hops_resolve() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, THREAD, &[THREAD]);
        vault.mail("mail/2026/quote.md", ACCOUNT, OTHER, &[OTHER]);
        let mut allowed = String::new();
        for hop in 0..8 {
            let next = hop + 1;
            let link = if hop == 0 { " [[mail/2026/lease]]" } else { "" };
            allowed.push_str(&format!(
                "- [-] #task Hop{hop}{link} ^s{hop}\n\
                 \t- 🪢 **LINKS:** merged into [[job#^s{next}]]\n"
            ));
        }
        allowed.push_str("- [x] #task Final ^s8\n");
        vault.write("job.md", &allowed);
        let mut capped = String::new();
        for hop in 0..9 {
            let next = hop + 1;
            let link = if hop == 0 { " [[mail/2026/quote]]" } else { "" };
            capped.push_str(&format!(
                "- [-] #task Cap{hop}{link} ^c{hop}\n\
                 \t- 🪢 **LINKS:** merged into [[deep#^c{next}]]\n"
            ));
        }
        capped.push_str("- [x] #task Too deep ^c9\n");
        vault.write("deep.md", &capped);
        let scan = vault.scan();
        let followed = evidence(&scan, THREAD);
        assert!(!followed.merge_unresolved());
        assert_eq!(followed.tasks[0].block_id.as_deref(), Some("s8"));
        assert!(scan.archivable(followed));
        let held = evidence(&scan, OTHER);
        assert_eq!(
            held.merge_unresolved.as_ref().map(|hold| hold.kind),
            Some(MergeUnresolvedKind::DepthCap)
        );
        assert!(!scan.archivable(held));
    }

    #[test]
    fn missing_block_and_ambiguous_basename_are_unresolved() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, THREAD, &[THREAD]);
        vault.mail("mail/2026/quote.md", ACCOUNT, OTHER, &[OTHER]);
        vault.write("areas/dest.md", "- [x] #task Area dest ^x\n");
        vault.write("projects/dest.md", "- [x] #task Project dest ^x\n");
        vault.write(
            "job.md",
            "- [-] #task Missing block [[mail/2026/lease]] ^src\n\
             \t- 🪢 **LINKS:** merged into [[job#^gone]]\n\
             - [-] #task Ambiguous [[mail/2026/quote]] ^amb\n\
             \t- 🪢 **LINKS:** merged into [[dest#^x]]\n",
        );
        let scan = vault.scan();
        let missing = evidence(&scan, THREAD);
        assert_eq!(
            missing.merge_unresolved.as_ref().map(|hold| hold.kind),
            Some(MergeUnresolvedKind::MissingBlock)
        );
        let ambiguous = evidence(&scan, OTHER);
        assert_eq!(
            ambiguous.merge_unresolved.as_ref().map(|hold| hold.kind),
            Some(MergeUnresolvedKind::AmbiguousBasename)
        );
    }

    #[test]
    fn live_and_done_sources_merged_into_a_closed_mail_linker_archive() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, THREAD, &[THREAD]);
        vault.mail("mail/2026/quote.md", ACCOUNT, OTHER, &[OTHER]);
        vault.write(
            "job.md",
            "- [-] #task Live source [[mail/2026/lease]] ^live\n\
             \t- 🪢 **LINKS:** merged into [[job#^dst]]\n\
             - [x] #task Shared target [[mail/2026/lease]] [[mail/2026/quote]] ^dst\n",
        );
        vault.write(
            "done/old.md",
            "- [-] #task Done source [[mail/2026/quote]] ^done\n\
             \t- 🪢 **LINKS:** merged into [[job#^dst]]\n",
        );
        let scan = vault.scan();
        let live = evidence(&scan, THREAD);
        assert!(!live.merge_unresolved());
        assert!(scan.archivable(live));
        let archived = evidence(&scan, OTHER);
        assert!(!archived.merge_unresolved());
        assert!(scan.archivable(archived));
    }

    #[test]
    fn saved_ids_and_fingerprints_are_captured_and_stable() {
        let mut vault = Vault::new();
        vault.mail(
            "mail/2026/lease.md",
            ACCOUNT,
            THREAD,
            &["ff02ff02ff02ff02", THREAD],
        );
        vault.write("job.md", "- [x] #task Done [[mail/2026/lease]]\n");
        let first = vault.scan();
        let before = evidence(&first, THREAD);
        assert_eq!(
            before.saved_ids,
            vec![THREAD.to_string(), "ff02ff02ff02ff02".to_string()]
        );
        assert!(first.archivable(before));
        let fingerprint = before.note_fingerprint.clone().unwrap();
        // A rescan freezes the same IDs and fingerprint.
        let second = vault.scan();
        let again = evidence(&second, THREAD);
        assert_eq!(again.saved_ids, before.saved_ids);
        assert_eq!(again.note_fingerprint, before.note_fingerprint);
        // User notes change the fingerprint but never the saved IDs.
        let edited =
            mail_note_text(ACCOUNT, THREAD, &["ff02ff02ff02ff02", THREAD])
                .replace("## Notes\n\n", "## Notes\n\nBryan's note.\n\n");
        vault.write("mail/2026/lease.md", &edited);
        let third = vault.scan();
        let edited_found = evidence(&third, THREAD);
        assert_eq!(edited_found.saved_ids, before.saved_ids);
        assert_ne!(edited_found.note_fingerprint, Some(fingerprint));
        assert!(third.archivable(edited_found));
    }

    #[test]
    fn pre_hydration_snapshot_excludes_ids_added_during_the_run() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, THREAD, &[THREAD]);
        vault.write("job.md", "- [x] #task Done [[mail/2026/lease]]\n");
        let before = vault.scan();
        let snapshot = before.saved_id_snapshot();
        assert!(snapshot.is_complete());

        vault.mail(
            "mail/2026/lease.md",
            ACCOUNT,
            THREAD,
            &[THREAD, "ff02ff02ff02ff02"],
        );
        let hydrated = vault.scan();
        assert_eq!(evidence(&hydrated, THREAD).saved_ids.len(), 2);

        let frozen = hydrated.with_saved_id_snapshot(&snapshot);
        assert_eq!(
            evidence(&frozen, THREAD).saved_ids,
            vec![THREAD.to_string()]
        );
        assert!(frozen.archivable(evidence(&frozen, THREAD)));
    }

    #[test]
    fn unreadable_notes_make_evidence_unsafe() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, THREAD, &[THREAD]);
        vault.write("job.md", "- [x] #task Done [[mail/2026/lease]]\n");
        std::fs::write(vault.root.join("bad.md"), [0xff, 0xfe, 0x00]).unwrap();
        let scan = vault.scan();
        assert_eq!(scan.incomplete_reads(), &["bad.md".to_string()]);
        assert!(!scan.scan_complete());
        let found = evidence(&scan, THREAD);
        assert!(found.issues.is_empty());
        assert!(!scan.archivable(found));
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_files_are_reported_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, THREAD, &[THREAD]);
        vault.write("job.md", "- [x] #task Done [[mail/2026/lease]]\n");
        vault.write("locked.md", "- [x] #task Locked\n");
        std::fs::set_permissions(
            vault.root.join("locked.md"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        if std::fs::read(vault.root.join("locked.md")).is_ok() {
            // Running as root: the mode bit does not block reads.
            return;
        }
        let scan = vault.scan();
        assert_eq!(scan.incomplete_reads(), &["locked.md".to_string()]);
        assert!(!scan.scan_complete());
    }

    #[test]
    fn rows_without_the_task_tag_never_count() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, THREAD, &[THREAD]);
        vault.write(
            "job.md",
            "- [x] Closed without the tag [[mail/2026/lease]]\n\
             - [ ] Open mention [[mail/2026/lease]]\n",
        );
        let scan = vault.scan();
        assert!(scan.evidence(&key(THREAD)).is_none());
    }

    #[test]
    fn issue_messages_are_body_free() {
        assert_eq!(
            ArchiveIssue::MissingMailNote {
                note: "mail/2026/gone.md".to_string(),
            }
            .message(),
            "mail note mail/2026/gone.md is indexed but missing"
        );
        assert!(!ArchiveIssue::DuplicateMailNote {
            paths: vec!["mail/2026/dup-a.md".to_string()],
        }
        .message()
        .contains("Dana"));
    }
}

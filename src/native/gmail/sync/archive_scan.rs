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
//!   [[note#Heading]]` entry under a `❌ **CANCEL LOG**` header) hold
//!   their thread for review: the cancellation proves a merge, not task
//!   completion. Only configured merge effects count, never prose
//!   substrings.
//! - Saved message IDs are parsed from the validated managed mail-note
//!   cards and frozen with a note fingerprint for later revalidation.
//!   Missing, duplicate, damaged, or mismatched mail notes are refusals,
//!   never zero watchers.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use sha2::{Digest, Sha256};

use super::scan::{
    fenced_lines, lower_stem, scan_note_tasks, DroppedKind, DroppedMailLink,
    LinkPlacement, MailLinks, OPEN_STATUSES,
};
use super::{
    super::{
        note::{index::MailIndex, parse as note_parse},
        vault_walk, GmailError,
    },
    ThreadKey,
};
use crate::native::{
    config::relations::RelationsConfig,
    done_archive, is_always_excluded_note_directory_name, markdown,
    relation_labels::relation_label_key,
    task_dependencies::blocks as blocks_mod,
    task_relations::{parse as relation_parse, LinksLine, RelationEntryKind},
};

/// Statuses that prove a task closed for archive: done or cancelled.
pub(crate) const POSITIVE_CLOSED: [char; 3] = ['x', 'X', '-'];

/// Section-merge provenance marker, `📥 merged into [[note#Heading]]`,
/// matched only inside Cancel Log children, never in prose.
const SECTION_MERGE_MARK: &str = "📥 merged into [[";

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
    /// At least one linker carries merge-retired provenance.
    pub(crate) merge_hold: bool,
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
    /// merge hold, no identity doubt, complete mail-note evidence with
    /// saved IDs, and a fully read vault.
    pub(crate) fn archivable(&self, evidence: &ArchiveThreadEvidence) -> bool {
        self.scan_complete()
            && !evidence.tasks.is_empty()
            && !evidence.has_open()
            && !evidence.has_unknown()
            && !evidence.positive_closed().is_empty()
            && !evidence.merge_hold
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
        let mut task_lines: BTreeSet<usize> = BTreeSet::new();
        for task in &found {
            task_lines.insert(task.line);
        }
        for link in &dropped {
            task_lines.insert(link.line);
        }
        let retired = merge_retired_lines(text, &task_lines, &merge_keys);
        for task in found {
            let line_retired = retired.contains(&task.line);
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
                        from_duplicate_note: false,
                    },
                );
            }
        }
        for link in dropped {
            match link.kind.clone() {
                DroppedKind::Duplicate(key) => {
                    let line_retired = retired.contains(&link.line);
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
        let merge_hold = linkers.iter().any(|task| task.merge_retired);
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
                tasks: linkers,
                saved_ids,
                note_fingerprint,
                merge_hold,
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

/// 1-based task-row lines carrying merge-retired provenance: a LINKS
/// child with a configured merge label, or a Cancel Log child with the
/// canonical section-merge marker. Both reuse the parsers the merge
/// writer is checked against; prose is never scanned.
fn merge_retired_lines(
    text: &str,
    task_lines: &BTreeSet<usize>,
    merge_keys: &BTreeSet<String>,
) -> BTreeSet<usize> {
    if task_lines.is_empty() || merge_keys.is_empty() {
        return BTreeSet::new();
    }
    let lines: Vec<&str> = text
        .split_inclusive('\n')
        .map(|line| markdown::split_line_ending(line).0)
        .collect();
    let body_start = markdown::strictly_closed_frontmatter_end(&lines)
        .map_or(0, |end| end + 1);
    let fenced = fenced_lines(&lines, body_start);
    task_lines
        .iter()
        .filter(|line| {
            let Some(index) = line.checked_sub(1) else {
                return false;
            };
            task_line_is_merge_retired(&lines, &fenced, index, merge_keys)
        })
        .copied()
        .collect()
}

/// Whether one 0-based task-row line carries merge-retired provenance.
fn task_line_is_merge_retired(
    lines: &[&str],
    fenced: &BTreeSet<usize>,
    task_index: usize,
    merge_keys: &BTreeSet<String>,
) -> bool {
    if lines.get(task_index).is_none() {
        return false;
    }
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
                return true;
            }
        }
    }
    // Section tombstones are dated entries beneath a direct-child
    // `❌ **CANCEL LOG**` header
    // (`- *2026-11-03* — 📥 merged into [[note#Heading]]`); a matching
    // marker in a prose child or a fenced block is never provenance.
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
                return true;
            }
        }
    }
    false
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
        assert!(!found.merge_hold);
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
    fn configured_merge_labels_hold_retired_sources() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, THREAD, &[THREAD]);
        vault.mail("mail/2026/quote.md", ACCOUNT, OTHER, &[OTHER]);
        vault.write(
            "job.md",
            "- [-] #task Retired [[mail/2026/lease]] ^src\n\
             \t- 🪢 **LINKS:** My Merge [[other#^x]]\n\
             - [x] #task Plain [[mail/2026/quote]] ^plain\n\
             \t- 🪢 **LINKS:** [[other#^y]]\n",
        );
        let relations = relations_with_merge(&["My Merge"]);
        let scan = vault.scan_with(&relations);
        let held = evidence(&scan, THREAD);
        assert_eq!(held.positive_closed().len(), 1);
        assert!(!held.has_open());
        assert!(held.merge_hold);
        assert!(held.tasks[0].merge_retired);
        assert!(!scan.archivable(held));
        // A label-less LINKS edge is not merge provenance.
        let plain = evidence(&scan, OTHER);
        assert!(!plain.merge_hold);
        assert!(!plain.tasks[0].merge_retired);
        assert!(scan.archivable(plain));
        // Without the configured label, the same edge does not hold.
        let unconfigured = vault.scan();
        assert!(!evidence(&unconfigured, THREAD).merge_hold);
    }

    #[test]
    fn fallback_merged_into_and_section_tombstones_hold() {
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
        // The default fallback label holds even with no configured merges,
        // and the hold needs no resolvable merge target.
        let fallback = evidence(&scan, THREAD);
        assert!(fallback.merge_hold);
        assert!(!scan.archivable(fallback));
        let section = evidence(&scan, OTHER);
        assert!(section.merge_hold);
        assert!(!scan.archivable(section));
        // Prose and fenced markers are never provenance.
        let prose = evidence(&scan, THIRD);
        assert_eq!(prose.tasks.len(), 2);
        assert!(!prose.merge_hold);
        assert!(scan.archivable(prose));
    }

    #[test]
    fn merge_hold_covers_open_sources_too() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, THREAD, &[THREAD]);
        vault.write(
            "job.md",
            "- [ ] #task Open with a merge edge [[mail/2026/lease]]\n\
             \t- 🪢 **LINKS:** merged into [[dest#^dst]]\n",
        );
        let scan = vault.scan();
        let found = evidence(&scan, THREAD);
        assert!(found.merge_hold);
        assert!(found.has_open());
        assert!(!scan.archivable(found));
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

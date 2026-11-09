//! Collection planning and plan application.
use super::*;

use super::super::task_status_hooks::{parse_tasks, task_group_classification};
use super::super::task_status_hooks_write::{
    planned_write, InputSnapshot, PlannedWrite, WritePlan,
};
use super::snapshot::{capture_error, capture_inputs, CapturedInputs};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CollectionPlan {
    pub(super) scanned_files: usize,
    pub(super) files: Vec<FilePlan>,
    pub(super) link_repairs: Vec<LinkRepairPlan>,
    pub(super) holds: Vec<HeldRoot>,
    pub(super) skip_counts: BTreeMap<String, usize>,
    pub(super) grandfathered: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct HeldRoot {
    pub(super) relative_path: PathBuf,
    pub(super) line: usize,
    pub(super) reason: &'static str,
    pub(super) block_ids: Vec<String>,
}

/// One selected archive root with its replay and rename outcome.
///
/// `raw_bytes` is the exact source block (root line plus children, with the
/// source line endings); `contained_ids` lists every block id inside it in
/// line order; `final_ids` lists the destination ids in the same order.
/// `replayed` roots already exist byte-identical in the archive: they are
/// removed from the source while the archive gains nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MovedRootRecord {
    pub(super) source_line: usize,
    pub(super) root_id: Option<String>,
    pub(super) raw_bytes: String,
    pub(super) contained_ids: Vec<String>,
    pub(super) final_ids: Vec<String>,
    pub(super) replayed: bool,
}

impl CollectionPlan {
    pub(super) fn is_empty(&self) -> bool {
        self.files.is_empty() && self.link_repairs.is_empty()
    }

    pub(super) fn total_task_count(&self) -> usize {
        self.files.iter().map(|file| file.task_count).sum()
    }

    pub(super) fn task_move_file_count(&self) -> usize {
        self.files.iter().filter(|file| file.task_count > 0).count()
    }

    pub(super) fn source_file_update_count(&self) -> usize {
        self.files
            .iter()
            .filter(|file| file.writes_source())
            .count()
    }

    pub(super) fn source_metadata_update_count(&self) -> usize {
        self.files
            .iter()
            .filter(|file| file.source_metadata_updated)
            .count()
    }

    pub(super) fn archive_metadata_update_count(&self) -> usize {
        self.files
            .iter()
            .filter(|file| file.archive_metadata_updated)
            .count()
    }

    pub(super) fn moved_block_id_count(&self) -> usize {
        self.files
            .iter()
            .map(|file| file.moved_block_ids.len())
            .sum()
    }

    pub(super) fn ambiguous_moved_block_id_count(&self) -> usize {
        self.files
            .iter()
            .map(|file| file.ambiguous_moved_block_ids.len())
            .sum()
    }

    pub(super) fn moved_block_id_rename_count(&self) -> usize {
        self.files
            .iter()
            .map(|file| file.moved_block_id_rename_count)
            .sum()
    }

    pub(super) fn link_repair_count(&self) -> usize {
        let file_repairs: usize = self
            .files
            .iter()
            .map(|file| {
                file.source_link_repair_count + file.archive_link_repair_count
            })
            .sum();
        let link_only_repairs: usize = self
            .link_repairs
            .iter()
            .map(|repair| repair.link_count)
            .sum();
        file_repairs + link_only_repairs
    }

    pub(super) fn dependency_metadata_repair_count(&self) -> usize {
        let file_repairs: usize = self
            .files
            .iter()
            .map(|file| {
                file.source_dependency_metadata_repair_count
                    + file.archive_dependency_metadata_repair_count
            })
            .sum();
        file_repairs
            + self
                .link_repairs
                .iter()
                .map(|repair| repair.dependency_metadata_count)
                .sum::<usize>()
    }

    pub(super) fn link_repair_file_count(&self) -> usize {
        let mut paths = BTreeSet::new();
        for file in &self.files {
            if file.source_link_repair_count > 0
                || file.source_dependency_metadata_repair_count > 0
            {
                paths.insert(file.relative_source_path.clone());
            }
            if file.archive_link_repair_count > 0
                || file.archive_dependency_metadata_repair_count > 0
            {
                paths.insert(file.relative_archive_path.clone());
            }
        }
        paths.extend(
            self.link_repairs
                .iter()
                .filter(|repair| {
                    repair.link_count > 0
                        || repair.dependency_metadata_count > 0
                })
                .map(|repair| repair.relative_path.clone()),
        );
        paths.len()
    }

    pub(super) fn planned_bytes(&self) -> usize {
        let file_bytes: usize = self
            .files
            .iter()
            .map(|file| {
                let source_bytes = if file.writes_source() {
                    file.source_contents.len()
                } else {
                    0
                };
                let archive_bytes = file
                    .archive_contents
                    .as_ref()
                    .map(String::len)
                    .unwrap_or(0);
                source_bytes + archive_bytes
            })
            .sum();
        let link_repair_bytes: usize = self
            .link_repairs
            .iter()
            .map(|repair| repair.contents.len())
            .sum();
        file_bytes + link_repair_bytes
    }
}

fn bump_skip(counts: &mut BTreeMap<String, usize>, reason: &str) {
    *counts.entry(reason.to_string()).or_insert(0) += 1;
}

fn merge_skips(
    counts: &mut BTreeMap<String, usize>,
    extra: &BTreeMap<String, usize>,
) {
    for (reason, count) in extra {
        *counts.entry(reason.clone()).or_insert(0) += count;
    }
}

fn has_type_frontmatter(contents: &str) -> bool {
    super::super::projects::parse_frontmatter(contents).is_some_and(
        |frontmatter| {
            !super::super::projects::frontmatter_type_names(&frontmatter)
                .is_empty()
        },
    )
}

/// Whether an absolute vault path names today's day file.
///
/// Compares the captured canonical identity first so symlinked checkouts
/// agree, then falls back to the plain absolute comparison. No vault reads:
/// the snapshot already carries the canonical path.
fn is_captured_daily(
    snapshot: &InputSnapshot,
    captured: &CapturedInputs,
) -> bool {
    if snapshot.path == captured.daily_path {
        return true;
    }
    match (snapshot.identity(), captured.daily_canonical.as_ref()) {
        (Some(identity), Some(daily_canonical)) => {
            identity.canonical_path == *daily_canonical
        }
        _ => false,
    }
}

fn is_daily_source(
    relative_path: &Path,
    snapshot: &InputSnapshot,
    captured: &CapturedInputs,
) -> bool {
    super::super::task_status_hooks::canonical_daily_date(relative_path)
        .is_some()
        || is_captured_daily(snapshot, captured)
}

enum SourceEligibility {
    Eligible { grandfathered: bool },
    Skip(&'static str),
}

fn classify_source(
    relative_path: &Path,
    snapshot: &InputSnapshot,
    contents: &str,
    captured: &CapturedInputs,
    archive_exists: bool,
) -> SourceEligibility {
    if !has_type_frontmatter(contents) && archive_exists {
        return SourceEligibility::Eligible {
            grandfathered: true,
        };
    }
    if is_daily_source(relative_path, snapshot, captured) {
        return SourceEligibility::Skip(SKIP_DAILY);
    }
    if super::super::projects::project_is_finished(relative_path, contents) {
        return SourceEligibility::Skip(SKIP_FINISHED_PROJECT);
    }
    if !grouping_eligible_for(relative_path, snapshot, contents, captured) {
        return SourceEligibility::Skip(SKIP_NOT_AREA_OR_PROJECT);
    }
    SourceEligibility::Eligible {
        grandfathered: false,
    }
}

/// Snapshot-aware copy of `grouping_eligible_note`.
///
/// Same rule — area/project frontmatter, not a canonical daily note, not
/// today's day file — with the daily comparison answered from the captured
/// canonical identity instead of a fresh filesystem canonicalization.
fn grouping_eligible_for(
    relative_path: &Path,
    snapshot: &InputSnapshot,
    contents: &str,
    captured: &CapturedInputs,
) -> bool {
    let is_area_or_project = super::super::projects::parse_frontmatter(
        contents,
    )
    .is_some_and(|frontmatter| {
        super::super::projects::frontmatter_is_area(&frontmatter)
            || super::super::projects::frontmatter_is_project(&frontmatter)
    });
    if !is_area_or_project {
        return false;
    }
    if super::super::task_status_hooks::canonical_daily_date(relative_path)
        .is_some()
    {
        return false;
    }
    !is_captured_daily(snapshot, captured)
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FilePlan {
    pub(super) relative_source_path: PathBuf,
    pub(super) relative_archive_path: PathBuf,
    pub(super) task_count: usize,
    pub(super) source_contents: String,
    pub(super) archive_contents: Option<String>,
    pub(super) source_metadata_updated: bool,
    pub(super) archive_metadata_updated: bool,
    pub(super) moved_block_ids: BTreeSet<String>,
    pub(super) ambiguous_moved_block_ids: BTreeSet<String>,
    pub(super) moved_block_id_final_ids: BTreeMap<String, String>,
    pub(super) moved_block_final_ids: BTreeSet<String>,
    pub(super) moved_block_id_rename_count: usize,
    pub(super) moved_roots: Vec<MovedRootRecord>,
    pub(super) source_structural: bool,
    pub(super) source_link_repair_count: usize,
    pub(super) archive_link_repair_count: usize,
    pub(super) source_dependency_metadata_repair_count: usize,
    pub(super) archive_dependency_metadata_repair_count: usize,
}

impl FilePlan {
    pub(super) fn writes_source(&self) -> bool {
        self.task_count > 0
            || self.source_metadata_updated
            || self.source_link_repair_count > 0
            || self.source_dependency_metadata_repair_count > 0
    }

    pub(super) fn writes_archive(&self) -> bool {
        self.archive_contents.is_some()
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LinkRepairPlan {
    pub(super) relative_path: PathBuf,
    pub(super) contents: String,
    pub(super) link_count: usize,
    pub(super) dependency_metadata_count: usize,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ArchiveWrite {
    Created,
    Updated,
}
pub(super) fn apply_file_plan(
    vault: &Path,
    file: &FilePlan,
) -> io::Result<Option<ArchiveWrite>> {
    let source_path = vault.join(&file.relative_source_path);
    let archive_path = vault.join(&file.relative_archive_path);
    let archive_write =
        if let Some(archive_contents) = file.archive_contents.as_deref() {
            let archive_write = if archive_path.is_file() {
                ArchiveWrite::Updated
            } else {
                ArchiveWrite::Created
            };
            atomic_write(&archive_path, archive_contents)?;
            Some(archive_write)
        } else {
            None
        };

    if file.writes_source() {
        atomic_write(&source_path, &file.source_contents)?;
    }

    Ok(archive_write)
}
pub(super) fn apply_link_repair_plan(
    vault: &Path,
    repair: &LinkRepairPlan,
) -> io::Result<()> {
    atomic_write(
        vault.join(&repair.relative_path).as_path(),
        &repair.contents,
    )
}
pub(super) fn build_collection_plan(
    vault: &Path,
    threshold: usize,
) -> io::Result<CollectionPlan> {
    let captured = capture_inputs(vault)?;
    plan_from_captured(&captured, threshold).map(|planned| planned.collection)
}

/// Build the guarded apply inputs for the archive collection.
///
/// Same captured pipeline as [`build_collection_plan`], returning the
/// [`WritePlan`] a later command applies under the maintenance lock: one
/// output per actually changed path, archive writes before source removals,
/// repair-only outputs last. Unstable captures surface as retryable
/// `ErrorKind::Interrupted` errors so the command rebuilds instead of
/// writing.
#[cfg_attr(not(test), allow(dead_code))] // Shared by the archive-command phase.
pub(super) fn build_write_plan(
    vault: &Path,
    threshold: usize,
) -> io::Result<WritePlan> {
    let captured = capture_inputs(vault)?;
    plan_from_captured(&captured, threshold).map(|planned| planned.write_plan)
}

pub(super) struct PlannedArchive {
    pub(super) collection: CollectionPlan,
    pub(super) write_plan: WritePlan,
}

fn plan_from_captured(
    captured: &CapturedInputs,
    threshold: usize,
) -> io::Result<PlannedArchive> {
    validate_dependency_identity_index(&captured.contents)?;
    let mut files = Vec::new();
    let mut holds = Vec::new();
    let mut skip_counts = BTreeMap::new();
    let mut grandfathered = Vec::new();

    for path in &captured.source_paths {
        let snapshot = snapshot_for_path(&captured.snapshots, path)?;
        let relative_source_path =
            vault_relative_path(&captured.vault, path, "note")?;
        let contents = captured.contents(&relative_source_path)?.to_string();
        let relative_archive_path =
            archive_relative_path(&relative_source_path)?;
        let archive_exists =
            captured.contents.contains_key(&relative_archive_path);
        match classify_source(
            &relative_source_path,
            snapshot,
            &contents,
            captured,
            archive_exists,
        ) {
            SourceEligibility::Skip(reason) => {
                bump_skip(&mut skip_counts, reason);
                continue;
            }
            SourceEligibility::Eligible {
                grandfathered: is_grandfathered,
            } => {
                if is_grandfathered {
                    grandfathered.push(relative_source_path.clone());
                }
            }
        }
        let transform =
            transform_markdown_with_settings(&contents, &captured.settings);
        merge_skips(&mut skip_counts, &transform.skip_counts);
        holds.extend(transform.holds.iter().map(|hold| HeldRoot {
            relative_path: relative_source_path.clone(),
            line: hold.line,
            reason: hold.reason,
            block_ids: hold.block_ids.clone(),
        }));
        let moves_tasks = transform.task_count >= threshold;
        let existing_archive = captured
            .contents
            .get(&relative_archive_path)
            .map(String::as_str);
        let partitioned = if moves_tasks {
            Some(partition_selected_roots(&transform, existing_archive)?)
        } else {
            None
        };

        let source_base = if moves_tasks {
            transform.source_contents
        } else {
            contents
        };
        let (source_contents, source_metadata_updated) =
            if moves_tasks || archive_exists {
                let link = archive_wiki_link(&relative_archive_path)?;
                let linked_source =
                    ensure_source_done_tasks_frontmatter(&source_base, &link);
                let source_metadata_updated = linked_source != source_base;
                (linked_source, source_metadata_updated)
            } else {
                (source_base, false)
            };
        let task_count = if moves_tasks { transform.task_count } else { 0 };
        let moved_block_ids = if moves_tasks {
            transform.moved_block_ids.clone()
        } else {
            BTreeSet::new()
        };
        let ambiguous_moved_block_ids = if moves_tasks {
            transform.ambiguous_moved_block_ids.clone()
        } else {
            BTreeSet::new()
        };
        let (
            archive_append,
            moved_block_id_final_ids,
            moved_block_final_ids,
            moved_block_id_rename_count,
            moved_roots,
        ) = match partitioned {
            Some(partitioned) => (
                partitioned.archive_append,
                partitioned.final_block_ids,
                partitioned.final_ids,
                partitioned.rename_count,
                partitioned.records,
            ),
            None => (
                String::new(),
                BTreeMap::new(),
                BTreeSet::new(),
                0,
                Vec::new(),
            ),
        };
        let (archive_contents, archive_metadata_updated) =
            if moves_tasks || archive_exists {
                let source_link = source_wiki_link(&relative_source_path)?;
                let append_sample = if archive_append.is_empty() {
                    existing_archive.unwrap_or("")
                } else {
                    &archive_append
                };
                let archive_base = archive_base_contents(
                    existing_archive,
                    append_sample,
                    &source_link,
                );
                let archive_metadata_updated = existing_archive
                    .map(|contents| archive_base != contents)
                    .unwrap_or(false);

                if !archive_append.is_empty() || archive_metadata_updated {
                    (
                        Some(archive_contents(
                            existing_archive,
                            &archive_append,
                            &source_link,
                        )),
                        archive_metadata_updated,
                    )
                } else {
                    (None, archive_metadata_updated)
                }
            } else {
                (None, false)
            };

        if task_count == 0
            && !source_metadata_updated
            && !archive_metadata_updated
        {
            continue;
        }

        files.push(FilePlan {
            relative_source_path,
            relative_archive_path,
            task_count,
            source_contents,
            archive_contents,
            source_metadata_updated,
            archive_metadata_updated,
            moved_block_ids,
            ambiguous_moved_block_ids,
            moved_block_id_final_ids,
            moved_block_final_ids,
            moved_block_id_rename_count,
            moved_roots,
            source_structural: false,
            source_link_repair_count: 0,
            archive_link_repair_count: 0,
            source_dependency_metadata_repair_count: 0,
            archive_dependency_metadata_repair_count: 0,
        });
    }

    let link_repairs =
        apply_link_repairs_to_plan(&captured.contents, &mut files)?;
    apply_same_write_grouping(captured, &mut files)?;
    verify_planned_postimages(captured, &files, &link_repairs)?;
    let write_plan = write_plan_for(captured, &files, &link_repairs)?;
    grandfathered.sort();
    holds.sort_by(|left, right| {
        left.relative_path
            .cmp(&right.relative_path)
            .then(left.line.cmp(&right.line))
    });

    Ok(PlannedArchive {
        collection: CollectionPlan {
            scanned_files: captured.source_paths.len(),
            files,
            link_repairs,
            holds,
            skip_counts,
            grandfathered,
        },
        write_plan,
    })
}

fn snapshot_for_path<'a>(
    snapshots: &'a [InputSnapshot],
    path: &Path,
) -> io::Result<&'a InputSnapshot> {
    snapshots
        .iter()
        .find(|snapshot| snapshot.path == path)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("missing captured snapshot for {}", path.display()),
            )
        })
}
pub(super) fn validate_dependency_identity_index(
    note_contents: &BTreeMap<PathBuf, String>,
) -> io::Result<()> {
    let mut identities: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut skipped = BTreeSet::new();
    for (relative_path, contents) in note_contents {
        for block_id in block_ids_in_markdown(contents) {
            let Ok(id) = dependency_id(relative_path, &block_id) else {
                skipped.insert(relative_path.clone());
                continue;
            };
            if let Some(existing_path) = identities.get(&id)
                && existing_path != relative_path
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "dependency identity collision {id}: {} and {}",
                        existing_path.display(),
                        relative_path.display()
                    ),
                ));
            }
            identities.insert(id, relative_path.clone());
        }
    }
    if !skipped.is_empty() {
        eprintln!(
            "{COMMAND_NAME}: warning: skipped dependency identities for notes with unsupported path characters: {}",
            skipped
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(())
}

struct PartitionedArchive {
    archive_append: String,
    final_block_ids: BTreeMap<String, String>,
    final_ids: BTreeSet<String>,
    rename_count: usize,
    records: Vec<MovedRootRecord>,
}

/// Split selected roots into exact replays and new archive appends.
///
/// A selected root replays when the existing archive already holds its exact
/// block bytes — same root line, same root id, same complete body. Replays
/// leave the archive untouched and take no rename; anything else flows
/// through the existing collision-renaming path. Comparison is byte-exact:
/// never a substring, id-only, or normalized-text match.
fn partition_selected_roots(
    transform: &Transform,
    existing_archive: Option<&str>,
) -> io::Result<PartitionedArchive> {
    let mut inventory = archive_block_inventory(existing_archive);
    let mut new_append = String::new();
    let mut new_occurrences = Vec::new();
    struct PendingRoot {
        source_line: usize,
        root_id: Option<String>,
        raw: String,
        contained: Vec<String>,
        replayed: bool,
        occurrence_base: usize,
        occurrence_count: usize,
    }
    let mut pending = Vec::new();
    for span in &transform.moved_roots {
        let raw = transform.archive_append[span.start..span.end].to_string();
        let contained = contained_block_ids(&raw);
        let replayed = match inventory.get_mut(&raw) {
            Some(count) if *count > 0 => {
                *count -= 1;
                true
            }
            _ => false,
        };
        let occurrence_base = new_occurrences.len();
        let mut occurrence_count = 0;
        if !replayed {
            let base = new_append.len();
            let mut line_offset = 0;
            for segment in raw.split_inclusive('\n') {
                let (content, _) = split_line_ending(segment);
                for occurrence in block_id_occurrences_in_text(content) {
                    new_occurrences.push(BlockIdOccurrence {
                        id: occurrence.id.clone(),
                        start: base + line_offset + occurrence.start,
                        end: base + line_offset + occurrence.end,
                    });
                    occurrence_count += 1;
                }
                line_offset += segment.len();
            }
            new_append.push_str(&raw);
        }
        pending.push(PendingRoot {
            source_line: span.source_line,
            root_id: span.root_id.clone(),
            raw,
            contained,
            replayed,
            occurrence_base,
            occurrence_count,
        });
    }
    let deduplicated = deduplicate_archive_append_block_ids(
        existing_archive,
        &new_append,
        &new_occurrences,
    );
    let mut final_block_ids = deduplicated.final_block_ids.clone();
    let mut final_ids = BTreeSet::new();
    final_ids.extend(deduplicated.aligned_final_ids.iter().cloned());
    let mut records = Vec::new();
    for pending in pending {
        let finals = if pending.replayed {
            for id in &pending.contained {
                if !transform.ambiguous_moved_block_ids.contains(id)
                    && !final_block_ids.contains_key(id)
                {
                    final_block_ids.insert(id.clone(), id.clone());
                }
                final_ids.insert(id.clone());
            }
            pending.contained.clone()
        } else {
            deduplicated.aligned_final_ids[pending.occurrence_base
                ..pending.occurrence_base + pending.occurrence_count]
                .to_vec()
        };
        records.push(MovedRootRecord {
            source_line: pending.source_line,
            root_id: pending.root_id,
            raw_bytes: pending.raw,
            contained_ids: pending.contained,
            final_ids: finals,
            replayed: pending.replayed,
        });
    }
    Ok(PartitionedArchive {
        archive_append: deduplicated.archive_append,
        final_block_ids,
        final_ids,
        rename_count: deduplicated.rename_count,
        records,
    })
}

/// Every block id on a moved root's lines, in line order.
fn contained_block_ids(raw: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for segment in raw.split_inclusive('\n') {
        let (content, _) = split_line_ending(segment);
        ids.extend(block_ids_in_text(content));
    }
    ids
}

/// Multiset of complete archive blocks, frontmatter excluded.
///
/// Blocks are segmented with the same [`task_block_end`] rule the source
/// transform uses, so an archived block and its source root segment
/// identically and compare byte-exact.
fn archive_block_inventory(
    existing_archive: Option<&str>,
) -> BTreeMap<String, usize> {
    let Some(existing) = existing_archive else {
        return BTreeMap::new();
    };
    let lines: Vec<&str> = existing.split_inclusive('\n').collect();
    let logical: Vec<&str> =
        lines.iter().map(|line| split_line_ending(line).0).collect();
    let mut start =
        match super::super::markdown::strictly_closed_frontmatter_end(&logical)
        {
            Some(end) => end + 1,
            None => 0,
        };
    let mut inventory = BTreeMap::new();
    while start < lines.len() {
        let (content, _) = split_line_ending(lines[start]);
        if content.trim().is_empty() {
            start += 1;
            continue;
        }
        let end = task_block_end(&lines, start, leading_indent_len(content));
        *inventory.entry(lines[start..end].concat()).or_insert(0) += 1;
        start = end;
    }
    inventory
}

/// Compose the final touched source contents with the status-group transform.
///
/// Only grouping-eligible notes are rearranged, and only when the source is
/// already changed (moves, metadata, or link repair). A changed postimage is
/// marked structural so the guarded writer applies its quiet period.
fn apply_same_write_grouping(
    captured: &CapturedInputs,
    files: &mut [FilePlan],
) -> io::Result<()> {
    let classification = task_group_classification(&captured.settings);
    for file in files.iter_mut() {
        if !file.writes_source() {
            continue;
        }
        let snapshot = snapshot_for_path(
            &captured.snapshots,
            &captured.vault.join(&file.relative_source_path),
        )?;
        let original = captured.contents(&file.relative_source_path)?;
        if !grouping_eligible_for(
            &file.relative_source_path,
            snapshot,
            original,
            captured,
        ) {
            continue;
        }
        let grouped = super::super::task_status_groups::transform(
            &file.source_contents,
            &classification,
        );
        if grouped.contents != file.source_contents {
            file.source_contents = grouped.contents;
            file.source_structural = true;
        }
    }
    Ok(())
}

/// Validate every final postimage before any write is installed.
///
/// Moved identities must resolve exactly once at their destination archive
/// and be gone from their source; proposed done-note postimages must hold no
/// open `#task`. Unrelated unchanged archives are never inspected. Any
/// violation is a plan-wide refusal: the plan is rebuilt, never patched.
fn verify_planned_postimages(
    captured: &CapturedInputs,
    files: &[FilePlan],
    link_repairs: &[LinkRepairPlan],
) -> io::Result<()> {
    let mut overlay = captured.contents.clone();
    for file in files {
        if file.writes_source() {
            overlay.insert(
                file.relative_source_path.clone(),
                file.source_contents.clone(),
            );
        }
        if let Some(archive_contents) = &file.archive_contents {
            overlay.insert(
                file.relative_archive_path.clone(),
                archive_contents.clone(),
            );
        }
    }
    for repair in link_repairs {
        overlay.insert(repair.relative_path.clone(), repair.contents.clone());
    }
    verify_moved_identities(&overlay, files)?;
    verify_archive_postimages(captured, &overlay, files, link_repairs)?;
    Ok(())
}

fn verify_moved_identities(
    overlay: &BTreeMap<PathBuf, String>,
    files: &[FilePlan],
) -> io::Result<()> {
    let mut census: BTreeMap<PathBuf, BTreeMap<String, Vec<usize>>> =
        BTreeMap::new();
    for (path, contents) in overlay {
        census.insert(path.clone(), anchor_id_lines(contents));
    }
    let note_paths: BTreeSet<PathBuf> = overlay.keys().cloned().collect();
    let targets = moved_block_targets(&note_paths, overlay, files)?;
    for file in files {
        if file.task_count == 0 {
            continue;
        }
        for record in &file.moved_roots {
            for final_id in &record.final_ids {
                let at_destination = census
                    .get(&file.relative_archive_path)
                    .and_then(|counts| counts.get(final_id))
                    .map(Vec::len)
                    .unwrap_or(0);
                if at_destination != 1 {
                    return Err(refusal(
                        &file.relative_archive_path,
                        record.source_line,
                        Some(final_id),
                        format!(
                            "moved block id ^{final_id} occurs {at_destination} times at its destination, expected exactly once"
                        ),
                    ));
                }
                let at_source = census
                    .get(&file.relative_source_path)
                    .and_then(|counts| counts.get(final_id))
                    .map(Vec::len)
                    .unwrap_or(0);
                if at_source != 0 {
                    return Err(refusal(
                        &file.relative_source_path,
                        record.source_line,
                        Some(final_id),
                        format!(
                            "moved block id ^{final_id} still occurs at its source"
                        ),
                    ));
                }
            }
            if let Some(root_id) = &record.root_id
                && !file.ambiguous_moved_block_ids.contains(root_id)
                && !record.replayed
            {
                let expected_final = record.final_ids.first().cloned();
                let resolved = targets
                    .get(&(file.relative_source_path.clone(), root_id.clone()));
                let matches = resolved.is_some_and(|target| {
                    Some(target.block_id.clone()) == expected_final
                        && target.archive_target
                            == vault_relative_link_target(
                                &file.relative_archive_path,
                                "archive",
                            )
                            .unwrap_or_default()
                });
                if !matches {
                    return Err(refusal(
                        &file.relative_source_path,
                        record.source_line,
                        Some(root_id),
                        "moved block id does not resolve to its destination archive"
                            .to_string(),
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Content-aware block-id census: one entry per anchor outside masked code,
/// comments, and frontmatter, skipping inline-code spans.
fn anchor_id_lines(contents: &str) -> BTreeMap<String, Vec<usize>> {
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();
    let masked = super::super::markdown::non_content_line_indexes(&lines);
    let mut census = BTreeMap::new();
    for (index, segment) in lines.iter().enumerate() {
        if masked.contains(&index) {
            continue;
        }
        let (content, _) = split_line_ending(segment);
        let code_spans =
            super::super::task_dependencies::inline_code_spans(content);
        for occurrence in block_id_occurrences_in_text(content) {
            if code_spans.iter().any(|span| {
                occurrence.start < span.end && occurrence.end > span.start
            }) {
                continue;
            }
            census
                .entry(occurrence.id)
                .or_insert_with(Vec::new)
                .push(index + 1);
        }
    }
    census
}

fn verify_archive_postimages(
    captured: &CapturedInputs,
    overlay: &BTreeMap<PathBuf, String>,
    files: &[FilePlan],
    link_repairs: &[LinkRepairPlan],
) -> io::Result<()> {
    let mut postimages: BTreeSet<&PathBuf> = BTreeSet::new();
    for file in files {
        if file.archive_contents.is_some() {
            postimages.insert(&file.relative_archive_path);
        }
    }
    for repair in link_repairs {
        if super::super::task_dependencies::is_archive_path(
            &repair.relative_path,
        ) {
            postimages.insert(&repair.relative_path);
        }
    }
    for path in postimages {
        let overlay_contents = overlay
            .get(path)
            .ok_or_else(|| missing_planned_contents(path))?;
        let lines: Vec<&str> = overlay_contents.split_inclusive('\n').collect();
        let masked = super::super::markdown::non_content_line_indexes(&lines);
        for task in parse_tasks(overlay_contents, &captured.settings) {
            if !task.status_type.is_open() {
                continue;
            }
            if masked.contains(&task.line_index) {
                continue;
            }
            let line = lines
                .get(task.line_index)
                .map(|segment| split_line_ending(segment).0)
                .unwrap_or("");
            if !has_task_tag(line) {
                continue;
            }
            return Err(refusal(
                path,
                task.line_index + 1,
                trailing_block_id_in_line(line),
                "proposed archive postimage holds an open #task".to_string(),
            ));
        }
    }
    Ok(())
}

fn refusal(
    path: &Path,
    line: usize,
    block_id: Option<impl Into<String>>,
    reason: String,
) -> io::Error {
    let id = block_id
        .map(|id| format!(" ^{}", id.into()))
        .unwrap_or_default();
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "{COMMAND_NAME}: refusing archive plan for {}:{line}{id}: {reason}",
            path.display()
        ),
    )
}

/// One guarded [`WritePlan`] output per actually changed path.
///
/// Archive writes come first so an interruption always leaves a recoverable
/// copy of every moved block, then source removals, then repair-only
/// outputs. Every output carries its captured original snapshot — including
/// missing snapshots for creates — so a vault that changed underfoot fails
/// the apply preflight instead of overwriting.
fn write_plan_for(
    captured: &CapturedInputs,
    files: &[FilePlan],
    link_repairs: &[LinkRepairPlan],
) -> io::Result<WritePlan> {
    let mut outputs: Vec<PlannedWrite> = Vec::new();
    for file in files {
        if let Some(archive_contents) = &file.archive_contents {
            let path = captured.vault.join(&file.relative_archive_path);
            let original = snapshot_for_path(&captured.snapshots, &path)?;
            outputs.push(
                planned_write(
                    path,
                    original,
                    archive_contents.as_bytes().to_vec(),
                    false,
                )
                .map_err(capture_error)?,
            );
        }
    }
    for file in files {
        if !file.writes_source() {
            continue;
        }
        let path = captured.vault.join(&file.relative_source_path);
        let original = snapshot_for_path(&captured.snapshots, &path)?;
        outputs.push(
            planned_write(
                path,
                original,
                file.source_contents.as_bytes().to_vec(),
                file.source_structural,
            )
            .map_err(capture_error)?,
        );
    }
    for repair in link_repairs {
        let path = captured.vault.join(&repair.relative_path);
        let original = snapshot_for_path(&captured.snapshots, &path)?;
        outputs.push(
            planned_write(
                path,
                original,
                repair.contents.as_bytes().to_vec(),
                false,
            )
            .map_err(capture_error)?,
        );
    }
    let mut scan_paths: Vec<PathBuf> = captured
        .source_paths
        .iter()
        .chain(captured.repair_paths.iter())
        .cloned()
        .collect();
    scan_paths.sort();
    scan_paths.dedup();
    Ok(WritePlan {
        vault_canonical: captured.vault_canonical.clone(),
        inputs: captured.snapshots.clone(),
        scan_paths,
        outputs,
    })
}

//! Collection planning and plan application.
use super::*;

use super::super::task_status_hooks::{parse_tasks, task_group_classification};
use super::super::task_status_hooks_write::{
    planned_remove, planned_write, InputSnapshot, PlannedWrite, WritePlan,
};
use super::health::{collect_diagnostics, Diagnostics};
use super::snapshot::{capture_error, capture_inputs, CapturedInputs};
use chrono::{Duration, NaiveDate};

/// One guarded source-rename maintenance: a bound noncanonical archive moves
/// to its canonical destination and the old path is removed recoverably.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RenamePlan {
    pub(super) source: PathBuf,
    pub(super) from: PathBuf,
    pub(super) to: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CollectionPlan {
    pub(super) scanned_files: usize,
    pub(super) files: Vec<FilePlan>,
    pub(super) link_repairs: Vec<LinkRepairPlan>,
    pub(super) renames: Vec<RenamePlan>,
    pub(super) holds: Vec<HeldRoot>,
    pub(super) skip_counts: BTreeMap<String, usize>,
    pub(super) grandfathered: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct HeldRoot {
    pub(super) relative_path: PathBuf,
    pub(super) line: usize,
    pub(super) reason: &'static str,
    /// The held root line's own trailing block id, when it carries one.
    pub(super) block_id: Option<String>,
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
        self.files.is_empty()
            && self.link_repairs.is_empty()
            && self.renames.is_empty()
    }

    pub(super) fn rename_count(&self) -> usize {
        self.renames.len()
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

    pub(super) fn link_repair_files(&self) -> BTreeSet<PathBuf> {
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
        paths
    }

    pub(super) fn link_repair_file_count(&self) -> usize {
        self.link_repair_files().len()
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

pub(super) enum SourceEligibility {
    Eligible { grandfathered: bool },
    Skip(&'static str),
}

pub(super) fn classify_source(
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
    /// Old archive path removed recoverably when this file moves a bound
    /// noncanonical archive to its canonical destination.
    pub(super) renamed_from: Option<PathBuf>,
    pub(super) task_count: usize,
    pub(super) source_contents: String,
    pub(super) archive_contents: Option<String>,
    /// The archive does not exist yet: its write is a create.
    pub(super) archive_created: bool,
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
    pub(super) structural_regrouping: bool,
}
/// Capture every planning input once and plan the whole archive collection.
///
/// The returned [`PlannedArchive`] carries the report-facing
/// [`CollectionPlan`] and the guarded [`WritePlan`] the command applies under
/// the maintenance lock: one output per actually changed path, archive writes
/// before source removals, repair-only outputs last. An unstable capture
/// surfaces as a retryable `ErrorKind::Interrupted` error so the command
/// rebuilds the plan instead of writing.
pub(super) fn plan_archive(
    vault: &Path,
    threshold: usize,
) -> io::Result<PlannedArchive> {
    let archive_config = super::super::config::load_archive_config(
        &super::super::config::config_path(),
    )
    .map_err(|error| {
        io::Error::new(io::ErrorKind::InvalidInput, error.to_string())
    })?;
    plan_archive_with_config(vault, threshold, archive_config)
}

fn plan_archive_with_config(
    vault: &Path,
    threshold: usize,
    archive_config: super::super::config::ArchiveConfig,
) -> io::Result<PlannedArchive> {
    let captured = capture_inputs(vault)?;
    plan_from_captured_with_config(&captured, threshold, archive_config)
}

#[cfg(test)]
pub(super) fn build_collection_plan(
    vault: &Path,
    threshold: usize,
) -> io::Result<CollectionPlan> {
    build_collection_plan_with_config(
        vault,
        threshold,
        super::super::config::ArchiveConfig::default(),
    )
}

#[cfg(test)]
pub(super) fn build_collection_plan_with_config(
    vault: &Path,
    threshold: usize,
    archive_config: super::super::config::ArchiveConfig,
) -> io::Result<CollectionPlan> {
    plan_archive_with_config(vault, threshold, archive_config)
        .map(|planned| planned.collection)
}

#[cfg(test)]
pub(super) fn build_planned_archive_with_config(
    vault: &Path,
    threshold: usize,
    archive_config: super::super::config::ArchiveConfig,
) -> io::Result<PlannedArchive> {
    plan_archive_with_config(vault, threshold, archive_config)
}

#[cfg(test)]
pub(super) fn build_write_plan(
    vault: &Path,
    threshold: usize,
) -> io::Result<WritePlan> {
    plan_archive(vault, threshold).map(|planned| planned.write_plan)
}

#[derive(Debug)]
pub(super) struct PlannedArchive {
    pub(super) collection: CollectionPlan,
    pub(super) write_plan: WritePlan,
    pub(super) diagnostics: Diagnostics,
}

#[cfg(test)]
pub(super) fn plan_from_captured(
    captured: &CapturedInputs,
    threshold: usize,
) -> io::Result<PlannedArchive> {
    plan_from_captured_with_config(
        captured,
        threshold,
        super::super::config::ArchiveConfig::default(),
    )
}

fn plan_from_captured_with_config(
    captured: &CapturedInputs,
    threshold: usize,
    archive_config: super::super::config::ArchiveConfig,
) -> io::Result<PlannedArchive> {
    let pairs = ArchivePairIndex::from_contents(&captured.contents);
    let diagnostics = collect_diagnostics(captured, &pairs);
    plan_from_captured_with(
        captured,
        &pairs,
        diagnostics,
        threshold,
        archive_config,
    )
}

fn plan_from_captured_with(
    captured: &CapturedInputs,
    pairs: &ArchivePairIndex,
    diagnostics: Diagnostics,
    threshold: usize,
    archive_config: super::super::config::ArchiveConfig,
) -> io::Result<PlannedArchive> {
    let result = plan_from_pairs(
        captured,
        pairs,
        diagnostics.clone(),
        threshold,
        archive_config,
    );
    result.map_err(|error| attach_diagnostics(error, diagnostics))
}

fn plan_from_pairs(
    captured: &CapturedInputs,
    pairs: &ArchivePairIndex,
    diagnostics: Diagnostics,
    threshold: usize,
    archive_config: super::super::config::ArchiveConfig,
) -> io::Result<PlannedArchive> {
    validate_dependency_identity_index(&captured.contents)?;
    let blocked_sources = diagnostics.write_blocked_sources();
    // Rename maintenance is resolved before eligibility can skip a source:
    // daily, reference, untyped, and finished-project sources keep archive
    // maintenance without becoming eligible for task extraction.
    let renames = collect_rename_targets(captured, pairs, &blocked_sources);
    let rename_by_source: BTreeMap<&PathBuf, &RenamePlan> = renames
        .iter()
        .map(|rename| (&rename.source, rename))
        .collect();
    let mut files = Vec::new();
    let mut holds = Vec::new();
    let mut skip_counts = BTreeMap::new();
    let mut grandfathered = Vec::new();

    for path in &captured.source_paths {
        let snapshot = snapshot_for_path(&captured.snapshots, path)?;
        let relative_source_path =
            vault_relative_path(&captured.vault, path, "note")?;
        if blocked_sources.contains(&relative_source_path) {
            continue;
        }
        let contents = captured.contents(&relative_source_path)?.to_string();
        let (relative_archive_path, archive_exists) =
            match pairs.for_source(&relative_source_path) {
                SourceLookup::Bound(bound) => (bound.archive, true),
                SourceLookup::Missing { canonical_archive } => {
                    let path = match canonical_archive {
                        Some(path) => path,
                        None => archive_relative_path(&relative_source_path)?,
                    };
                    (path, false)
                }
                SourceLookup::Refused { .. } => continue,
            };
        // A bound noncanonical archive moves to its canonical destination;
        // the old path's bytes stay the append base and are removed later.
        let renamed_from = rename_by_source
            .get(&relative_source_path)
            .map(|rename| rename.from.clone());
        let relative_archive_path =
            match rename_by_source.get(&relative_source_path) {
                Some(rename) => rename.to.clone(),
                None => relative_archive_path,
            };
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
        let candidates =
            transform_markdown_with_settings(&contents, &captured.settings);
        merge_skips(&mut skip_counts, &candidates.skip_counts);
        holds.extend(candidates.holds.iter().map(|hold| {
            HeldRoot {
                relative_path: relative_source_path.clone(),
                line: hold.line,
                reason: hold.reason,
                block_id: contents
                    .lines()
                    .nth(hold.line.saturating_sub(1))
                    .and_then(trailing_block_id_in_line),
                block_ids: hold.block_ids.clone(),
            }
        }));
        // A rename reads the old archive as its base while installing the
        // canonical destination; every other plan reads its destination.
        let existing_archive = match &renamed_from {
            Some(from) => captured.contents.get(from).map(String::as_str),
            None => captured
                .contents
                .get(&relative_archive_path)
                .map(String::as_str),
        };
        let destination_exists =
            captured.contents.contains_key(&relative_archive_path);
        let note_keep_cap = note_archive_keep(
            &contents,
            &relative_source_path,
            archive_config.keep_max,
        );
        let selected_lines = select_archive_roots(
            &candidates,
            existing_archive,
            super::super::env::current_datetime().date(),
            archive_config.keep_days,
            note_keep_cap,
        );
        let moves_tasks = !selected_lines.is_empty()
            && (threshold == 0 || selected_lines.len() >= threshold);
        let transform = if moves_tasks {
            Some(transform_markdown_with_selected_roots(
                &contents,
                &captured.settings,
                Some(&selected_lines),
            ))
        } else {
            None
        };
        let partitioned = match &transform {
            Some(transform) => {
                Some(partition_selected_roots(transform, existing_archive)?)
            }
            None => None,
        };

        let source_base = transform
            .as_ref()
            .map(|transform| transform.source_contents.clone())
            .unwrap_or(contents);
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
        let task_count = transform
            .as_ref()
            .map(|transform| transform.task_count)
            .unwrap_or(0);
        let moved_block_ids = transform
            .as_ref()
            .map(|transform| transform.moved_block_ids.clone())
            .unwrap_or_default();
        let ambiguous_moved_block_ids = transform
            .as_ref()
            .map(|transform| transform.ambiguous_moved_block_ids.clone())
            .unwrap_or_default();
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

        // A rename always installs its canonical destination, even when the
        // copied frontmatter already names the right source: the path move
        // itself is the change. Refusing an occupied destination stays with
        // the pair index, which never reports such a pair as bound.
        let is_rename = renamed_from.is_some();
        if task_count == 0
            && !source_metadata_updated
            && !archive_metadata_updated
            && !is_rename
        {
            continue;
        }
        let archive_contents = if is_rename && archive_contents.is_none() {
            let source_link = source_wiki_link(&relative_source_path)?;
            let base = match existing_archive {
                Some(old) => {
                    archive_base_contents(Some(old), old, &source_link)
                }
                None => archive_base_contents(None, "", &source_link),
            };
            Some(base)
        } else {
            archive_contents
        };
        let archive_metadata_updated =
            match (&existing_archive, &archive_contents) {
                (_, None) => archive_metadata_updated,
                (Some(old), Some(next)) if is_rename => next.as_str() != *old,
                _ => archive_metadata_updated,
            };

        files.push(FilePlan {
            relative_source_path,
            relative_archive_path,
            renamed_from,
            task_count,
            source_contents,
            archive_created: archive_contents.is_some() && !destination_exists,
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

    plan_rename_only_files(
        captured,
        pairs,
        &blocked_sources,
        &renames,
        &mut files,
    )?;
    let mut link_repairs = apply_link_repairs_to_plan(
        &captured.contents,
        &mut files,
        pairs,
        &blocked_sources,
        &renames,
    )?;
    apply_same_write_grouping(captured, &mut files, &mut link_repairs)?;
    verify_planned_postimages(
        captured,
        &files,
        &link_repairs,
        pairs,
        &blocked_sources,
        &renames,
    )?;
    let write_plan = write_plan_for(captured, &files, &link_repairs, &renames)?;
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
            renames,
            holds,
            skip_counts,
            grandfathered,
        },
        write_plan,
        diagnostics,
    })
}

/// Bound noncanonical archives resolved before eligibility filtering.
///
/// Daily, reference, untyped, and finished-project sources keep this
/// maintenance without task extraction. Occupied canonical destinations are
/// never overwritten: the pair index reports those as refused, and a
/// canonical path present in the census is skipped here as well so
/// diagnosing duplicates never deletes a copy.
fn collect_rename_targets(
    captured: &CapturedInputs,
    pairs: &ArchivePairIndex,
    blocked_sources: &BTreeSet<PathBuf>,
) -> Vec<RenamePlan> {
    let mut renames = Vec::new();
    for (source, lookup) in pairs.source_entries() {
        let SourceLookup::Bound(bound) = lookup else {
            continue;
        };
        if bound.archive == bound.canonical_archive {
            continue;
        }
        if blocked_sources.contains(source) {
            continue;
        }
        if captured.contents.contains_key(&bound.canonical_archive) {
            continue;
        }
        if !captured.contents.contains_key(&bound.archive) {
            continue;
        }
        renames.push(RenamePlan {
            source: source.to_path_buf(),
            from: bound.archive.clone(),
            to: bound.canonical_archive.clone(),
        });
    }
    renames.sort_by(|left, right| {
        left.source
            .cmp(&right.source)
            .then(left.from.cmp(&right.from))
    });
    renames
}

/// FilePlans for renames whose source needed no task extraction.
///
/// The source loop above only visits eligibility-passing notes through
/// `captured.source_paths`; a rename whose source was skipped there (daily,
/// finished, untyped, or below threshold) still moves its archive bytes,
/// repairs its managed `parent` / `type` / `done_tasks` fields, and removes
/// the old path. Body, custom frontmatter, line endings, and block ids are
/// preserved verbatim; only the managed fields change.
fn plan_rename_only_files(
    captured: &CapturedInputs,
    pairs: &ArchivePairIndex,
    blocked_sources: &BTreeSet<PathBuf>,
    renames: &[RenamePlan],
    files: &mut Vec<FilePlan>,
) -> io::Result<()> {
    for rename in renames {
        if files
            .iter()
            .any(|file| file.relative_source_path == rename.source)
        {
            continue;
        }
        if blocked_sources.contains(&rename.source) {
            continue;
        }
        // Conflicted bindings stay out: only bound pairs reach `renames`,
        // but a concurrent refusal must not gain a maintenance write.
        if !matches!(pairs.for_source(&rename.source), SourceLookup::Bound(_)) {
            continue;
        }
        let source_original = captured.contents(&rename.source)?.to_string();
        let old_archive = captured.contents(&rename.from)?.to_string();
        let archive_link = archive_wiki_link(&rename.to)?;
        let source_link = source_wiki_link(&rename.source)?;
        let source_contents = ensure_source_done_tasks_frontmatter(
            &source_original,
            &archive_link,
        );
        let source_metadata_updated = source_contents != source_original;
        let archive_contents = archive_base_contents(
            Some(&old_archive),
            &old_archive,
            &source_link,
        );
        let archive_metadata_updated = archive_contents != old_archive;
        files.push(FilePlan {
            relative_source_path: rename.source.clone(),
            relative_archive_path: rename.to.clone(),
            renamed_from: Some(rename.from.clone()),
            task_count: 0,
            source_contents,
            archive_created: true,
            archive_contents: Some(archive_contents),
            source_metadata_updated,
            archive_metadata_updated,
            moved_block_ids: BTreeSet::new(),
            ambiguous_moved_block_ids: BTreeSet::new(),
            moved_block_id_final_ids: BTreeMap::new(),
            moved_block_final_ids: BTreeSet::new(),
            moved_block_id_rename_count: 0,
            moved_roots: Vec::new(),
            source_structural: false,
            source_link_repair_count: 0,
            archive_link_repair_count: 0,
            source_dependency_metadata_repair_count: 0,
            archive_dependency_metadata_repair_count: 0,
        });
    }
    files.sort_by(|left, right| {
        left.relative_source_path.cmp(&right.relative_source_path)
    });
    Ok(())
}

pub(super) fn snapshot_for_path<'a>(
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

/// Resolve the source note's optional archive keep cap. `None` means `off`;
/// invalid explicit values warn once and fall back to the global default.
fn note_archive_keep(
    contents: &str,
    relative_path: &Path,
    default_cap: u32,
) -> Option<usize> {
    use super::super::projects::FrontmatterScalar;

    let Some(frontmatter) = super::super::projects::parse_frontmatter(contents)
    else {
        return Some(default_cap as usize);
    };
    let scalar = super::super::projects::frontmatter_scalar(
        &frontmatter,
        "archive_keep",
    );
    let raw = match scalar {
        FrontmatterScalar::Absent => return Some(default_cap as usize),
        FrontmatterScalar::Blank => String::new(),
        FrontmatterScalar::Value(value) => value,
        FrontmatterScalar::NonScalar(value) => value,
    };
    if raw.eq_ignore_ascii_case("off") {
        return None;
    }
    if let Ok(number) = raw.parse::<u32>()
        && (1..=999).contains(&number)
    {
        return Some(number as usize);
    }
    eprintln!(
        "{COMMAND_NAME}: warning: archive_keep {raw:?} in {} is not 1–999 or off; using archive.keep_max ({default_cap})",
        relative_path.display()
    );
    Some(default_cap as usize)
}

/// Select roots outside the configured trailing calendar window, plus
/// exact archive replays that only need their source copy removed after an
/// interrupted run. Undated roots count as oldest; future-dated roots stay.
fn select_archive_roots(
    transform: &Transform,
    existing_archive: Option<&str>,
    today: NaiveDate,
    keep_days: u32,
    keep_cap: Option<usize>,
) -> BTreeSet<usize> {
    let cutoff = today - Duration::days(i64::from(keep_days - 1));
    let mut inventory = archive_block_inventory(existing_archive);
    let mut selected = BTreeSet::new();
    let mut recent = Vec::new();

    for span in &transform.moved_roots {
        let raw = &transform.archive_append[span.start..span.end];
        let is_replay = match inventory.get_mut(raw) {
            Some(count) if *count > 0 => {
                *count -= 1;
                true
            }
            _ => false,
        };
        if is_replay {
            selected.insert(span.source_line);
            continue;
        }

        let close_date = raw
            .lines()
            .next()
            .and_then(super::super::ref_tasks::close_date)
            .and_then(|date| NaiveDate::parse_from_str(&date, "%Y-%m-%d").ok());
        match close_date {
            None if keep_cap.is_some() => {
                selected.insert(span.source_line);
            }
            Some(date) if date < cutoff && keep_cap.is_some() => {
                selected.insert(span.source_line);
            }
            Some(date) if date <= today && keep_cap.is_some() => {
                recent.push((date, span.source_line));
            }
            // Future-dated roots and every non-replay root under `off` stay.
            Some(_) | None => {}
        }
    }

    recent.sort_by(|(left_date, left_line), (right_date, right_line)| {
        right_date
            .cmp(left_date)
            .then_with(|| right_line.cmp(left_line))
    });
    if let Some(keep_cap) = keep_cap {
        selected
            .extend(recent.into_iter().skip(keep_cap).map(|(_, line)| line));
    }
    selected
}

fn compare_month_keys(left: &str, right: &str) -> std::cmp::Ordering {
    match (left == "Undated", right == "Undated") {
        (true, true) => std::cmp::Ordering::Equal,
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        (false, false) => left.cmp(right),
    }
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
        close_month: String,
        append_start: usize,
        append_end: usize,
        occurrence_base: usize,
        occurrence_count: usize,
    }
    let mut pending = Vec::new();
    for span in &transform.moved_roots {
        let raw = transform.archive_append[span.start..span.end].to_string();
        let occurrences = content_anchor_occurrences(&raw);
        let contained = occurrences
            .iter()
            .map(|occurrence| occurrence.id.clone())
            .collect::<Vec<_>>();
        let replayed = match inventory.get_mut(&raw) {
            Some(count) if *count > 0 => {
                *count -= 1;
                true
            }
            _ => false,
        };
        let occurrence_base = new_occurrences.len();
        let mut occurrence_count = 0;
        let append_start = new_append.len();
        if !replayed {
            let base = new_append.len();
            for occurrence in &occurrences {
                new_occurrences.push(BlockIdOccurrence {
                    id: occurrence.id.clone(),
                    start: base + occurrence.start,
                    end: base + occurrence.end,
                });
                occurrence_count += 1;
            }
            new_append.push_str(&raw);
        }
        let append_end = new_append.len();
        let close_month = raw
            .lines()
            .next()
            .and_then(super::super::ref_tasks::close_date)
            .and_then(|date| NaiveDate::parse_from_str(&date, "%Y-%m-%d").ok())
            .map(|date| date.format("%Y-%m").to_string())
            .unwrap_or_else(|| "Undated".to_string());
        pending.push(PendingRoot {
            source_line: span.source_line,
            root_id: span.root_id.clone(),
            raw,
            contained,
            replayed,
            close_month,
            append_start,
            append_end,
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
    let mut month_appends: BTreeMap<String, Vec<(usize, String)>> =
        BTreeMap::new();
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
            let final_start = remap_append_offset(
                pending.append_start,
                &new_occurrences,
                &deduplicated.aligned_final_ids,
            );
            let final_end = remap_append_offset(
                pending.append_end,
                &new_occurrences,
                &deduplicated.aligned_final_ids,
            );
            let final_root =
                &deduplicated.archive_append[final_start..final_end];
            let final_root = rewrite_renamed_pathless_links(
                final_root,
                &deduplicated.final_block_ids,
            );
            month_appends
                .entry(pending.close_month.clone())
                .or_default()
                .push((pending.source_line, final_root));
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
    let line_ending = preferred_line_ending(&new_append);
    let mut archive_append = String::new();
    let mut month_appends = month_appends.into_iter().collect::<Vec<_>>();
    month_appends
        .sort_by(|(left, _), (right, _)| compare_month_keys(left, right));
    for (month, mut roots) in month_appends {
        roots.sort_by_key(|(line, _)| *line);
        archive_append.push_str("## ");
        archive_append.push_str(&month);
        archive_append.push_str(line_ending);
        archive_append.push_str(line_ending);
        for (_, root) in roots {
            archive_append.push_str(&root);
            if !root.ends_with('\n') {
                archive_append.push_str(line_ending);
            }
        }
    }
    Ok(PartitionedArchive {
        archive_append,
        final_block_ids,
        final_ids,
        rename_count: deduplicated.rename_count,
        records,
    })
}

/// Convert a byte offset in the pre-deduplicated append to the matching
/// offset after block-id suffixes were applied.
fn remap_append_offset(
    offset: usize,
    occurrences: &[BlockIdOccurrence],
    final_ids: &[String],
) -> usize {
    let delta = occurrences
        .iter()
        .zip(final_ids)
        .filter(|(occurrence, _)| occurrence.end <= offset)
        .map(|(occurrence, final_id)| {
            final_id.len() as isize
                - (occurrence.end - occurrence.start) as isize
        })
        .sum::<isize>();
    offset
        .checked_add_signed(delta)
        .expect("valid remapped offset")
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
    link_repairs: &mut [LinkRepairPlan],
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
        // Finished projects keep rename maintenance without task-group
        // rearrangement: only their managed `done_tasks` line may change.
        if super::super::projects::project_is_finished(
            &file.relative_source_path,
            original,
        ) {
            continue;
        }
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
    for repair in link_repairs {
        let snapshot = snapshot_for_path(
            &captured.snapshots,
            &captured.vault.join(&repair.relative_path),
        )?;
        let original = captured.contents(&repair.relative_path)?;
        if !grouping_eligible_for(
            &repair.relative_path,
            snapshot,
            original,
            captured,
        ) {
            continue;
        }
        let grouped = super::super::task_status_groups::transform(
            &repair.contents,
            &classification,
        );
        if grouped.contents != repair.contents {
            repair.contents = grouped.contents;
            repair.structural_regrouping = true;
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
    pairs: &ArchivePairIndex,
    blocked_sources: &BTreeSet<PathBuf>,
    renames: &[RenamePlan],
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
    // Old rename paths are removed by the plan: verify the overlay without
    // them, so no surviving reference may point at a removed archive.
    for rename in renames {
        overlay.remove(&rename.from);
    }
    verify_moved_identities(captured, &overlay, files, pairs, blocked_sources)?;
    verify_archive_postimages(captured, &overlay, files, link_repairs)?;
    verify_rename_references(captured, &overlay, renames)?;
    Ok(())
}

/// No source-of-truth dependency link may point to a removed archive.
///
/// Every anchor the old archive held keeps its block id at the canonical
/// destination with a new path-qualified dependency id. Any surviving
/// `[id::]` / `[dependsOn::]` token naming an old dependency id is a plan
/// refusal, as is a renamed destination that lost an archived anchor.
fn verify_rename_references(
    captured: &CapturedInputs,
    overlay: &BTreeMap<PathBuf, String>,
    renames: &[RenamePlan],
) -> io::Result<()> {
    for rename in renames {
        let old_archive = captured.contents(&rename.from)?.to_string();
        let old_ids = content_anchor_ids(&old_archive);
        let Some(destination) = overlay.get(&rename.to) else {
            return Err(refusal(
                &rename.to,
                1,
                None::<String>,
                format!(
                    "renamed archive {} has no planned contents",
                    rename.to.display()
                ),
            ));
        };
        let destination_ids: BTreeSet<String> =
            content_anchor_ids(destination).into_iter().collect();
        for id in &old_ids {
            if !destination_ids.contains(id) {
                return Err(refusal(
                    &rename.to,
                    1,
                    Some(id),
                    format!(
                        "renamed archive lost archived block id ^{id} from {}",
                        rename.from.display()
                    ),
                ));
            }
        }
        let mut stale_ids = BTreeSet::new();
        for id in &old_ids {
            let Ok(old_dependency) = dependency_id(&rename.from, id) else {
                continue;
            };
            let Ok(new_dependency) = dependency_id(&rename.to, id) else {
                continue;
            };
            if old_dependency != new_dependency {
                stale_ids.insert(old_dependency);
            }
        }
        if stale_ids.is_empty() {
            continue;
        }
        for (path, contents) in overlay {
            for captures in DEPENDENCY_FIELD_RE.captures_iter(contents) {
                let key =
                    captures.name("key").map_or("", |value| value.as_str());
                let value =
                    captures.name("value").map_or("", |value| value.as_str());
                let hits_stale = if key == "id" {
                    stale_ids.contains(value.trim())
                } else if key == "dependsOn" {
                    value
                        .split(',')
                        .any(|segment| stale_ids.contains(segment.trim()))
                } else {
                    false
                };
                if hits_stale {
                    return Err(refusal(
                        path,
                        1,
                        None::<String>,
                        format!(
                            "dependency metadata still points to removed archive {}",
                            rename.from.display()
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

pub(super) fn verify_moved_identities(
    captured: &CapturedInputs,
    overlay: &BTreeMap<PathBuf, String>,
    files: &[FilePlan],
    pairs: &ArchivePairIndex,
    blocked_sources: &BTreeSet<PathBuf>,
) -> io::Result<()> {
    let mut census: BTreeMap<PathBuf, BTreeMap<String, Vec<usize>>> =
        BTreeMap::new();
    for (path, contents) in overlay {
        census.insert(path.clone(), anchor_id_lines(contents));
    }
    let note_paths: BTreeSet<PathBuf> = overlay.keys().cloned().collect();
    let targets = moved_block_targets(
        &note_paths,
        overlay,
        files,
        pairs,
        blocked_sources,
    )?;
    for file in files {
        if file.task_count == 0 {
            continue;
        }
        let original_source = captured.contents(&file.relative_source_path)?;
        let original_source_census = anchor_id_lines(original_source);
        let source_census = census
            .get(&file.relative_source_path)
            .expect("source belongs to overlay");
        let mut moved_original_counts = BTreeMap::new();
        for record in &file.moved_roots {
            if record.contained_ids.len() != record.final_ids.len() {
                return Err(refusal(
                    &file.relative_source_path,
                    record.source_line,
                    record.root_id.clone(),
                    "moved block identity alignment is incomplete".to_string(),
                ));
            }
            for original_id in &record.contained_ids {
                *moved_original_counts
                    .entry(original_id.clone())
                    .or_insert(0usize) += 1;
            }
            for (original_id, final_id) in
                record.contained_ids.iter().zip(&record.final_ids)
            {
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
                if !file.ambiguous_moved_block_ids.contains(original_id) {
                    let resolved = targets.get(&(
                        file.relative_source_path.clone(),
                        original_id.clone(),
                    ));
                    let matches = resolved.is_some_and(|target| {
                        target.block_id == *final_id
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
                            Some(original_id),
                            "moved block id does not resolve to its destination archive"
                                .to_string(),
                        ));
                    }
                }
            }
        }
        for (original_id, moved_count) in moved_original_counts {
            let original_count = original_source_census
                .get(&original_id)
                .map(Vec::len)
                .unwrap_or(0);
            if moved_count > original_count {
                return Err(refusal(
                    &file.relative_source_path,
                    1,
                    Some(&original_id),
                    format!(
                        "moved block id ^{original_id} has {moved_count} recorded occurrences but only {original_count} at its source"
                    ),
                ));
            }
            let expected_remaining = original_count - moved_count;
            let actual_remaining =
                source_census.get(&original_id).map(Vec::len).unwrap_or(0);
            if actual_remaining != expected_remaining {
                return Err(refusal(
                    &file.relative_source_path,
                    1,
                    Some(&original_id),
                    format!(
                        "moved block id ^{original_id} has {actual_remaining} occurrences at its source after moving {moved_count}; expected {expected_remaining}"
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Content-aware block-id census: one entry per anchor outside masked code,
/// comments, and frontmatter, skipping inline-code spans.
fn anchor_id_lines(contents: &str) -> BTreeMap<String, Vec<usize>> {
    let mut census = BTreeMap::new();
    for occurrence in content_anchor_occurrences(contents) {
        census
            .entry(occurrence.id)
            .or_insert_with(Vec::new)
            .push(occurrence.line_index + 1);
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

/// A validation refusal: the proposed plan violates an archive invariant.
///
/// Distinguished from capture and I/O errors so the command can report a
/// plan-wide refusal (no retry, no writes) instead of a generic failure.
#[derive(Debug)]
pub(super) struct PlanRefusal(String);

impl std::fmt::Display for PlanRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PlanRefusal {}

pub(super) fn is_plan_refusal(error: &io::Error) -> bool {
    error.get_ref().is_some_and(|inner| {
        inner.downcast_ref::<PlanRefusal>().is_some()
            || inner
                .downcast_ref::<CarriedPlanError>()
                .is_some_and(|carried| carried.refused)
    })
}

/// Diagnostics collected before a planning error, when the census finished.
pub(super) fn carried_diagnostics(error: &io::Error) -> Option<&Diagnostics> {
    error.get_ref().and_then(|inner| {
        inner
            .downcast_ref::<CarriedPlanError>()
            .map(|carried| &carried.diagnostics)
    })
}

#[derive(Debug)]
struct CarriedPlanError {
    message: String,
    refused: bool,
    diagnostics: Diagnostics,
}

impl std::fmt::Display for CarriedPlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CarriedPlanError {}

fn attach_diagnostics(error: io::Error, diagnostics: Diagnostics) -> io::Error {
    let refused = is_plan_refusal(&error);
    io::Error::new(
        error.kind(),
        CarriedPlanError {
            message: error.to_string(),
            refused,
            diagnostics,
        },
    )
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
        PlanRefusal(format!(
            "refusing archive plan for {}:{line}{id}: {reason}",
            path.display()
        )),
    )
}

/// One guarded [`WritePlan`] output per actually changed path.
///
/// Destination archives install first so an interruption always leaves a
/// recoverable copy of every moved block, then source and repair outputs,
/// then guarded old-path removals. Every output carries its captured
/// original snapshot — including missing snapshots for creates — so a vault
/// that changed underfoot fails the apply preflight instead of overwriting.
/// A partial apply stops without automatic retry: both copies and the
/// recovery manifest stay available when a deletion did not complete, and a
/// later run refuses the unresolved duplicate claims instead of guessing
/// which copy is authoritative.
fn write_plan_for(
    captured: &CapturedInputs,
    files: &[FilePlan],
    link_repairs: &[LinkRepairPlan],
    renames: &[RenamePlan],
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
                repair.structural_regrouping,
            )
            .map_err(capture_error)?,
        );
    }
    // Guarded old-path removals install last with their captured before-images
    // in the recovery manifest. Each changed path is emitted once: the old
    // path never appears in any other output.
    let mut seen_removals = BTreeSet::new();
    for rename in renames {
        if !seen_removals.insert(rename.from.clone()) {
            continue;
        }
        let path = captured.vault.join(&rename.from);
        let original = snapshot_for_path(&captured.snapshots, &path)?;
        outputs.push(planned_remove(path, original).map_err(capture_error)?);
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
        vault_path: captured.vault.clone(),
        vault_canonical: captured.vault_canonical.clone(),
        inputs: captured.snapshots.clone(),
        scan_paths,
        outputs,
    })
}

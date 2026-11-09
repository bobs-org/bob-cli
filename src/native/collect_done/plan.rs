//! Collection planning and plan application.
use super::*;

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

fn paths_equal(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn is_daily_source(
    relative_path: &Path,
    absolute_path: &Path,
    daily_path: &Path,
) -> bool {
    super::super::task_status_hooks::canonical_daily_date(relative_path)
        .is_some()
        || paths_equal(absolute_path, daily_path)
}

enum SourceEligibility {
    Eligible { grandfathered: bool },
    Skip(&'static str),
}

fn classify_source(
    relative_path: &Path,
    absolute_path: &Path,
    contents: &str,
    daily_path: &Path,
    archive_exists: bool,
) -> SourceEligibility {
    if !has_type_frontmatter(contents) && archive_exists {
        return SourceEligibility::Eligible {
            grandfathered: true,
        };
    }
    if is_daily_source(relative_path, absolute_path, daily_path) {
        return SourceEligibility::Skip(SKIP_DAILY);
    }
    if super::super::projects::project_is_finished(relative_path, contents) {
        return SourceEligibility::Skip(SKIP_FINISHED_PROJECT);
    }
    if !super::super::task_status_hooks::grouping_eligible_note(
        relative_path,
        absolute_path,
        contents,
        daily_path,
    ) {
        return SourceEligibility::Skip(SKIP_NOT_AREA_OR_PROJECT);
    }
    SourceEligibility::Eligible {
        grandfathered: false,
    }
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
    let markdown_files = markdown_files(vault)?;
    let note_contents = markdown_files
        .iter()
        .map(|path| {
            fs::read_to_string(path).map(|contents| (path.clone(), contents))
        })
        .collect::<io::Result<BTreeMap<_, _>>>()?;
    validate_dependency_identity_index(vault, &note_contents)?;
    let settings = super::super::task_status_hooks::read_tasks_settings(vault);
    let daily_path = super::super::pomodoro::day_file_for(vault);
    let mut files = Vec::new();
    let mut holds = Vec::new();
    let mut skip_counts = BTreeMap::new();
    let mut grandfathered = Vec::new();

    for path in &markdown_files {
        let contents = note_contents
            .get(path)
            .expect("all markdown files were read")
            .clone();
        let relative_source_path = path
            .strip_prefix(vault)
            .map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "source path {} is outside vault {}: {error}",
                        path.display(),
                        vault.display()
                    ),
                )
            })?
            .to_path_buf();
        let relative_archive_path =
            archive_relative_path(&relative_source_path)?;
        let archive_path = vault.join(&relative_archive_path);
        let archive_exists = archive_path.is_file();
        match classify_source(
            &relative_source_path,
            path,
            &contents,
            &daily_path,
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
        let transform = transform_markdown_with_settings(&contents, &settings);
        merge_skips(&mut skip_counts, &transform.skip_counts);
        holds.extend(transform.holds.iter().map(|hold| HeldRoot {
            relative_path: relative_source_path.clone(),
            line: hold.line,
            reason: hold.reason,
        }));
        let moves_tasks = transform.task_count >= threshold;
        let existing_archive = read_optional_string(&archive_path)?;
        let archive_exists = existing_archive.is_some();
        let deduplicated_archive_append = if moves_tasks {
            Some(deduplicate_archive_append_block_ids(
                existing_archive.as_deref(),
                &transform.archive_append,
                &transform.moved_block_id_occurrences,
            ))
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
            transform.moved_block_ids
        } else {
            BTreeSet::new()
        };
        let ambiguous_moved_block_ids = if moves_tasks {
            transform.ambiguous_moved_block_ids
        } else {
            BTreeSet::new()
        };
        let (
            archive_append,
            moved_block_id_final_ids,
            moved_block_id_rename_count,
        ) = if let Some(deduplicated_archive_append) =
            deduplicated_archive_append
        {
            (
                deduplicated_archive_append.archive_append,
                deduplicated_archive_append.final_block_ids,
                deduplicated_archive_append.rename_count,
            )
        } else {
            (String::new(), BTreeMap::new(), 0)
        };
        let moved_block_final_ids = if moves_tasks {
            block_ids_in_markdown(&archive_append).into_iter().collect()
        } else {
            BTreeSet::new()
        };
        let (archive_contents, archive_metadata_updated) =
            if moves_tasks || archive_exists {
                let source_link = source_wiki_link(&relative_source_path)?;
                let archive_base = archive_base_contents(
                    existing_archive.as_deref(),
                    &archive_append,
                    &source_link,
                );
                let archive_metadata_updated = existing_archive
                    .as_deref()
                    .map(|contents| archive_base != contents)
                    .unwrap_or(false);

                if moves_tasks || archive_metadata_updated {
                    (
                        Some(archive_contents(
                            existing_archive.as_deref(),
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
            source_link_repair_count: 0,
            archive_link_repair_count: 0,
            source_dependency_metadata_repair_count: 0,
            archive_dependency_metadata_repair_count: 0,
        });
    }

    let link_repairs = apply_link_repairs_to_plan(vault, &mut files)?;
    grandfathered.sort();
    holds.sort_by(|left, right| {
        left.relative_path
            .cmp(&right.relative_path)
            .then(left.line.cmp(&right.line))
    });

    Ok(CollectionPlan {
        scanned_files: markdown_files.len(),
        files,
        link_repairs,
        holds,
        skip_counts,
        grandfathered,
    })
}
pub(super) fn validate_dependency_identity_index(
    vault: &Path,
    note_contents: &BTreeMap<PathBuf, String>,
) -> io::Result<()> {
    let mut identities: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut skipped = BTreeSet::new();
    for (absolute_path, contents) in note_contents {
        let relative_path = vault_relative_path(vault, absolute_path, "note")?;
        for block_id in block_ids_in_markdown(contents) {
            let Ok(id) = dependency_id(&relative_path, &block_id) else {
                skipped.insert(relative_path.clone());
                continue;
            };
            if let Some(existing_path) = identities.get(&id)
                && existing_path != &relative_path
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

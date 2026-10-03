//! Output composition and task-grouping eligibility.
use super::*;

pub(super) fn compose_outputs(
    files: &[FileScan],
    changes: &[PlannedChange],
    ctx: ComposeContext<'_>,
    extra_structural: &BTreeSet<usize>,
    original_contents: &BTreeMap<usize, String>,
) -> ComposeResult {
    let mut by_file: BTreeMap<usize, Vec<&PlannedChange>> = BTreeMap::new();
    for change in changes {
        by_file.entry(change.file_index).or_default().push(change);
    }
    let mut updated = BTreeMap::new();
    for (file_index, mut file_changes) in by_file {
        let file = &files[file_index];
        file_changes.sort_by_key(|change| change.status_byte_offset);
        let mut contents = file.contents.clone();
        for change in file_changes.into_iter().rev() {
            let offset = change.status_byte_offset;
            let status_len = contents[offset..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(1);
            contents.replace_range(
                offset..offset + status_len,
                &change.replacement.to_string(),
            );
        }
        updated.insert(file_index, contents);
    }

    let canonical_daily_path = ctx.daily_path.canonicalize().ok();
    let daily_file_index = files.iter().position(|file| {
        file.path == ctx.daily_path
            || canonical_daily_path.as_ref().is_some_and(|daily| {
                file.path.canonicalize().ok().as_ref() == Some(daily)
            })
    });
    // `files` already carries the reconciled contents, so a daily
    // note with no status change still keeps its adoptions and target
    // stamps; falling back to the pre-reconcile normalized contents
    // would report those edits but never write them.
    let daily_base = daily_file_index
        .and_then(|index| {
            updated
                .get(&index)
                .map(String::as_str)
                .or(Some(files[index].contents.as_str()))
        })
        .unwrap_or(ctx.normalized_daily_contents);
    let updated_daily = daily_base.to_string();
    let external_daily = if let Some(index) = daily_file_index {
        updated.insert(index, updated_daily);
        None
    } else {
        Some(updated_daily)
    };

    let classification = task_group_classification(ctx.settings);
    let mut grouped_task_sections = Vec::new();
    let mut grouping_warnings = Vec::new();
    let mut structural_files = BTreeSet::new();
    for (index, file) in files.iter().enumerate() {
        if !task_grouping_eligible(
            file,
            ctx.daily_path,
            ctx.previous_daily_path,
        ) {
            continue;
        }
        let input = updated
            .get(&index)
            .map(String::as_str)
            .unwrap_or(&file.contents);
        let transformed = task_status_groups::transform(input, &classification);
        grouped_task_sections.extend(
            transformed
                .grouped_sections
                .iter()
                .map(|section| grouped_section_report(file, section)),
        );
        grouping_warnings.extend(
            transformed
                .warnings
                .iter()
                .map(|warning| grouping_warning_report(file, warning)),
        );
        if transformed.changed {
            structural_files.insert(index);
            updated.insert(index, transformed.contents);
        }
    }

    // Dependency projection edits count as structural, so notes
    // modified less than the quiet interval ago defer through the
    // normal guarded-write path (`contract` §4.1, R10).
    structural_files.extend(extra_structural.iter().copied());
    // Projection-only notes have no status change: seed them so their
    // reconciled contents still reach the guarded write.
    for index in extra_structural {
        updated
            .entry(*index)
            .or_insert_with(|| files[*index].contents.clone());
    }
    let mut outputs = updated
        .into_iter()
        .filter_map(|(index, contents)| {
            let original_contents = if Some(index) == daily_file_index {
                ctx.daily_contents
            } else {
                original_contents
                    .get(&index)
                    .map(String::as_str)
                    .unwrap_or(&files[index].contents)
            };
            (contents != original_contents).then(|| ComposedOutput {
                path: files[index].path.clone(),
                contents,
                structural_regrouping: structural_files.contains(&index),
            })
        })
        .collect::<Vec<_>>();
    if let Some(updated_daily) = external_daily
        && updated_daily != ctx.daily_contents
    {
        outputs.push(ComposedOutput {
            path: ctx.daily_path.to_path_buf(),
            contents: updated_daily,
            structural_regrouping: false,
        });
    }

    ComposeResult {
        outputs,
        grouped_task_sections,
        grouping_warnings,
    }
}

pub(crate) fn task_group_classification(
    settings: &TasksSettings,
) -> TaskClassification {
    TaskClassification::from_status_types(
        &settings.global_filter,
        settings.status_types.iter().map(|(symbol, status_type)| {
            (*symbol, status_type.as_tasks_type_name())
        }),
        '*',
        '/',
        '?',
        ' ',
    )
}

pub(super) fn task_grouping_eligible(
    file: &FileScan,
    daily_path: &Path,
    previous_daily_path: Option<&Path>,
) -> bool {
    if !grouping_eligible_note(
        &file.relative_path,
        &file.path,
        &file.contents,
        daily_path,
    ) {
        return false;
    }
    if previous_daily_path.is_some_and(|path| paths_match(&file.path, path)) {
        return false;
    }
    true
}

/// Whether a note's contents are grouping-eligible: `[[area]]` or
/// `[[project]]` frontmatter, not a canonical `YYYY/YYYYMMDD.md` daily note,
/// and not today's day file. Shared with `bob randomize`, which composes
/// status and grouping in the same write so hooks have nothing left to do.
pub(crate) fn grouping_eligible_note(
    relative_path: &Path,
    path: &Path,
    contents: &str,
    daily_path: &Path,
) -> bool {
    if !note_kind(contents).is_area_or_project() {
        return false;
    }
    if canonical_daily_date(relative_path).is_some()
        || paths_match(path, daily_path)
    {
        return false;
    }
    true
}

pub(super) fn paths_match(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

pub(super) fn grouped_section_report(
    file: &FileScan,
    section: &GroupedSection,
) -> GroupedTaskSectionReport {
    GroupedTaskSectionReport {
        path: display_path(&file.relative_path),
        original_heading_line: section.original_heading_line,
        heading_ancestry: section.heading_ancestry.clone(),
        open: section.open,
        next_and_in_progress: section.next_and_in_progress,
        blocked: section.blocked,
        done_and_canceled: section.done_and_canceled,
        moved_block_count: section.moved_block_count,
        moved_blocks: section
            .moved_blocks
            .iter()
            .map(|block| GroupedMovedBlockReport {
                original_line: block.original_line,
                destination: block.destination.as_str().to_string(),
            })
            .collect(),
    }
}

pub(super) fn grouping_warning_report(
    file: &FileScan,
    warning: &GroupingWarning,
) -> GroupingWarningReport {
    GroupingWarningReport {
        path: display_path(&file.relative_path),
        original_heading_line: warning.original_heading_line,
        heading_ancestry: warning.heading_ancestry.clone(),
        code: warning.code.as_str().to_string(),
        message: warning.message.clone(),
    }
}

pub(super) fn apply_guarded_outputs(
    vault: &Path,
    inputs: &[InputSnapshot],
    scan_paths: &[PathBuf],
    outputs: Vec<ComposedOutput>,
) -> Result<ApplyReport, SyncError> {
    let mut planned = Vec::new();
    for output in outputs {
        let original =
            snapshot_for_path(inputs, &output.path).ok_or_else(|| {
                SyncError::new(format!(
                    "planned write is missing a snapshot: {}",
                    output.path.display()
                ))
            })?;
        planned.push(
            planned_write(
                output.path,
                original,
                output.contents.into_bytes(),
                output.structural_regrouping,
            )
            .map_err(|error| capture_to_sync("plan note write", error))?,
        );
    }
    let vault_canonical =
        vault.canonicalize().unwrap_or_else(|_| vault.to_path_buf());
    let rescan_vault = vault.to_path_buf();
    let session = ApplySession::production(Box::new(move || {
        markdown_files(&rescan_vault)
    }));
    match apply_plan(
        &WritePlan {
            vault_canonical,
            inputs: inputs.to_vec(),
            scan_paths: scan_paths.to_vec(),
            outputs: planned,
        },
        &session,
    ) {
        Ok(ApplyOutcome::NoOp) => Ok(ApplyReport::default()),
        Ok(ApplyOutcome::Applied {
            applied_files,
            recovery_directory,
        }) => Ok(ApplyReport {
            applied_files: applied_files
                .iter()
                .map(|path| display_report_path(vault, path))
                .collect(),
            deferred_files: Vec::new(),
            recovery_directory: Some(recovery_directory.display().to_string()),
        }),
        Err(error) => Err(SyncError::from_apply_with_vault(error, vault)),
    }
}

pub(super) fn display_report_path(vault: &Path, path: &Path) -> String {
    path.strip_prefix(vault)
        .map(display_path)
        .unwrap_or_else(|_| path.display().to_string())
}

pub(super) fn push_unique_input(
    inputs: &mut Vec<InputSnapshot>,
    snapshot: InputSnapshot,
) {
    if snapshot_for_path(inputs, &snapshot.path).is_some() {
        return;
    }
    inputs.push(snapshot);
}

pub(super) fn required_utf8(
    snapshot: &InputSnapshot,
) -> Result<String, CaptureError> {
    snapshot
        .utf8_contents()?
        .ok_or_else(|| CaptureError::NotFound(snapshot.path.clone()))
}

pub(super) fn capture_to_sync(action: &str, error: CaptureError) -> SyncError {
    let reason = match &error {
        CaptureError::Unstable(_) => Some("unstable_read"),
        CaptureError::Unsupported { .. } => Some("unsupported_file"),
        CaptureError::InvalidUtf8(_) => Some("io"),
        _ => None,
    };
    let mut sync_error =
        SyncError::new(format!("failed to {action} {}", error.message()));
    sync_error.reason = reason.map(str::to_string);
    sync_error
}

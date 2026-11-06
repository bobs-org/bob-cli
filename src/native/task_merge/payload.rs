use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use super::{
    super::{
        capture,
        collect_done::{
            repair_links_in_note_with, vault_relative_link_target,
            MovedBlockTarget, MovedBlockTargets,
        },
        note_tasks, task_children, task_complete, task_dependencies,
        task_locator::{self, LocatedTask},
        task_move::{
            transplant::{self, LineRange},
            IdCollisionPolicy, VaultSnapshot,
        },
    },
    containers::{self, ResolvedDestination},
    types::{
        locator_text, MergeContext, MergeError, MergeRequest, PreparedMerge,
        PreparedPayload, PreparedSource, SourceDisposition,
    },
};

struct SourceWork {
    prepared: PreparedSource,
    located: LocatedTask,
    start_offset: usize,
    child_ranges: Vec<LineRange>,
    child_fragments: Vec<(String, String)>,
}

struct RawPayload {
    source_index: usize,
    source_path: PathBuf,
    contents: String,
}

/// Validate source identities and structural relationships for the complete
/// request, including sources that are already closed. This deliberately does
/// not resolve or create the destination container, so an unchanged retry can
/// remain harmless when the original container has since disappeared.
pub(crate) fn validate_merge_request(
    snapshot: &VaultSnapshot,
    request: &MergeRequest,
    context: &MergeContext<'_>,
) -> Result<(), MergeError> {
    if request.sources.is_empty() {
        return Err(MergeError::new(
            "merge_no_sources",
            "select at least one source task",
        ));
    }

    let mut sources = Vec::with_capacity(request.sources.len());
    let mut identities = BTreeSet::new();
    for locator in &request.sources {
        let located = task_locator::resolve_task(
            &snapshot.notes,
            &locator_text(locator),
            context.task_settings,
        )
        .map_err(|error| MergeError::new(error.code, error.message))?;
        let identity = (
            located.path.clone(),
            located.task.block_id.clone().unwrap_or_default(),
        );
        if !identities.insert(identity) {
            return Err(MergeError::new(
                "merge_duplicate_source",
                format!(
                    "source ^{} was selected more than once",
                    locator.block_id
                ),
            ));
        }
        let lines =
            capture::line_spans(note_contents(snapshot, &located.path)?);
        let source_line = lines
            .get(located.task.line_index)
            .map(|line| line.text)
            .ok_or_else(|| {
                MergeError::new(
                    "task_not_found",
                    format!(
                        "no task `^{}` in `{}`",
                        locator.block_id,
                        located.path.display()
                    ),
                )
            })?;
        if task_complete::is_recurring_task_line(source_line) {
            return Err(MergeError::new(
                "merge_source_recurring",
                format!("^{} is recurring", locator.block_id),
            ));
        }
        if located.task.block_id.as_deref() == Some("prj") {
            return Err(MergeError::new(
                "merge_source_project",
                "the ^prj task cannot be merged",
            ));
        }
        sources.push(SourceWork {
            prepared: PreparedSource {
                locator: locator.clone(),
                path: located.path.clone(),
                line: located.line,
                block_id: located.task.block_id.clone().unwrap_or_default(),
                title: String::new(),
                previous_status: located.task.status_symbol,
                disposition: SourceDisposition::from(&located),
                moved_lines: 0,
                moved_notes: 0,
            },
            start_offset: line_start_offset(&lines, located.task.line_index),
            located,
            child_ranges: Vec::new(),
            child_fragments: Vec::new(),
        });
    }

    let target_task = target_task(snapshot, &request.target, context)?;
    validate_target_and_source_ranges(
        snapshot,
        &sources,
        target_task.as_ref(),
    )?;
    for source in &sources {
        source_payload(
            note_contents(snapshot, &source.located.path)?,
            &source.located,
            context,
        )?;
    }
    Ok(())
}

/// Prepare destination payloads and anchor/link repair without retiring
/// sources or applying dependency, cancellation, and offer effects.
pub(crate) fn prepare_task_merge(
    snapshot: &VaultSnapshot,
    request: &MergeRequest,
    context: &MergeContext<'_>,
) -> Result<PreparedMerge, MergeError> {
    if request.sources.is_empty() {
        return Err(MergeError::new(
            "merge_no_sources",
            "select at least one source task",
        ));
    }

    let mut sources = Vec::with_capacity(request.sources.len());
    let mut identities = BTreeSet::new();
    for locator in &request.sources {
        let located = task_locator::resolve_task(
            &snapshot.notes,
            &locator_text(locator),
            context.task_settings,
        )
        .map_err(|error| MergeError::new(error.code, error.message))?;
        let identity = (located.path.clone(), locator.block_id.clone());
        if !identities.insert(identity) {
            return Err(MergeError::new(
                "merge_duplicate_source",
                format!(
                    "source ^{} was selected more than once",
                    locator.block_id
                ),
            ));
        }
        let source_lines =
            capture::line_spans(note_contents(snapshot, &located.path)?);
        let source_line = source_lines
            .get(located.task.line_index)
            .map(|line| line.text)
            .ok_or_else(|| {
                MergeError::new(
                    "task_not_found",
                    format!(
                        "no task `^{}` in `{}`",
                        locator.block_id,
                        located.path.display()
                    ),
                )
            })?;
        if task_complete::is_recurring_task_line(source_line) {
            return Err(MergeError::new(
                "merge_source_recurring",
                format!("^{} is recurring", locator.block_id),
            ));
        }
        if located.task.block_id.as_deref() == Some("prj") {
            return Err(MergeError::new(
                "merge_source_project",
                "the ^prj task cannot be merged",
            ));
        }
        let disposition = SourceDisposition::from(&located);
        let prepared = PreparedSource {
            locator: locator.clone(),
            path: located.path.clone(),
            line: located.line,
            block_id: locator.block_id.clone(),
            title: note_tasks::clean_description(
                &located.task.description,
                &context.task_settings.global_filter,
                located.task.block_id.as_deref(),
            ),
            previous_status: located.task.status_symbol,
            disposition,
            moved_lines: 0,
            moved_notes: 0,
        };
        let start_offset =
            line_start_offset(&source_lines, located.task.line_index);
        sources.push(SourceWork {
            prepared,
            located,
            start_offset,
            child_ranges: Vec::new(),
            child_fragments: Vec::new(),
        });
    }

    if sources
        .iter()
        .all(|source| source.prepared.disposition == SourceDisposition::Closed)
    {
        return Ok(PreparedMerge {
            container: None,
            sources: sources
                .into_iter()
                .map(|source| source.prepared)
                .collect(),
            payloads: Vec::new(),
            postimages: BTreeMap::new(),
            renamed_block_ids: Vec::new(),
            repaired_files: Vec::new(),
            read_paths: snapshot.notes.keys().cloned().collect(),
            warnings: Vec::new(),
            insertion_dest: None,
            insertion_origins: Vec::new(),
        });
    }

    let initial_destination =
        containers::resolve_destination(snapshot, &request.target, context)?;
    let target_task = target_task(snapshot, &request.target, context)?;

    validate_target_and_source_ranges(
        snapshot,
        &sources,
        target_task.as_ref(),
    )?;

    // Resolve destination relationships before inspecting source children so a
    // target nested in a source gets its specific refusal rather than the
    // broader open-subtasks refusal.
    for source in &mut sources {
        if source.prepared.disposition == SourceDisposition::Closed {
            continue;
        }
        let (child_ranges, child_fragments, moved_notes) = source_payload(
            note_contents(snapshot, &source.located.path)?,
            &source.located,
            context,
        )?;
        source.prepared.moved_lines = child_ranges
            .iter()
            .map(|range| range.end.saturating_sub(range.start))
            .sum();
        source.prepared.moved_notes = moved_notes;
        source.child_ranges = child_ranges;
        source.child_fragments = child_fragments;
    }

    let mut source_ranges: BTreeMap<PathBuf, Vec<LineRange>> = BTreeMap::new();
    for source in &sources {
        if source.prepared.disposition == SourceDisposition::Closed {
            continue;
        }
        source_ranges
            .entry(source.prepared.path.clone())
            .or_default()
            .extend(source.child_ranges.iter().cloned());
    }
    let mut staged = snapshot.clone();
    for (path, ranges) in &source_ranges {
        let original = note_contents(snapshot, path)?;
        let retained = transplant::remove_line_ranges(original, ranges);
        staged.notes.insert(path.clone(), retained);
    }

    // Resolve against staged bytes. Source cuts may have shifted every saved
    // line and byte offset in a shared destination note.
    let staged_destination =
        containers::resolve_destination(&staged, &request.target, context)?;
    staged.notes.insert(
        staged_destination.container.path.clone(),
        staged_destination.contents.clone(),
    );

    let note_index = context
        .note_index
        .with_additional(snapshot.notes.keys().cloned());
    let raw_payloads = build_payloads(&sources, &staged_destination);
    let raw_fragments: Vec<String> = raw_payloads
        .iter()
        .map(|payload| payload.contents.clone())
        .collect();
    let destination_ids: BTreeSet<String> =
        transplant::live_block_ids(&staged_destination.contents)
            .into_iter()
            .collect();
    let (renamed, renamed_block_ids, _) = transplant::rename_fragments(
        &raw_fragments,
        &staged_destination.container.path,
        &destination_ids,
        IdCollisionPolicy::Rename,
    )
    .map_err(|error| match error {
        transplant::RenameError::Collision { block_id, .. } => MergeError::new(
            "merge_anchor_collision",
            format!("could not relocate descendant anchor {block_id}"),
        ),
    })?;

    let source_id_maps = source_id_maps(&raw_payloads, &renamed)?;
    let moved_targets = moved_targets(
        &raw_payloads,
        &renamed,
        &staged_destination.container.path,
    )?;
    let mut prepared_payloads = Vec::with_capacity(renamed.len());
    let mut payload_link_count = 0;
    let mut payload_dependency_count = 0;
    for (raw, fragment) in raw_payloads.iter().zip(renamed.iter()) {
        let source_index = raw.source_index;
        let source = &sources[source_index];
        let source_target =
            vault_relative_link_target(&source.prepared.path, "merge source")
                .map_err(|error| {
                MergeError::new("merge_invalid_path", error.to_string())
            })?;
        let id_map = source_id_maps
            .get(&source.prepared.path)
            .cloned()
            .unwrap_or_default();
        let stayed_source_ids = staged
            .notes
            .get(&source.prepared.path)
            .map(|contents| {
                transplant::live_block_ids(contents)
                    .into_iter()
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        let repaired = transplant::repair_moved_fragment(
            fragment,
            &source.prepared.path,
            &source_target,
            &note_index,
            &moved_targets,
            &stayed_source_ids,
            &id_map,
        );
        payload_link_count += repaired.link_count;
        payload_dependency_count += repaired.dependency_count;
        prepared_payloads.push(PreparedPayload {
            source_index,
            source_path: raw.source_path.clone(),
            contents: repaired.contents,
        });
    }

    let repaired_destination = repair_links_in_note_with(
        &staged_destination.contents,
        &staged_destination.container.path,
        &note_index,
        &moved_targets,
        true,
    );
    staged.notes.insert(
        staged_destination.container.path.clone(),
        repaired_destination.contents.clone(),
    );
    let mut final_destination =
        containers::resolve_destination(&staged, &request.target, context)?;
    let staged_created = match &staged_destination.container.kind {
        super::types::PreparedContainerKind::Drawer { created, .. }
        | super::types::PreparedContainerKind::Section { created, .. } => {
            *created
        }
        _ => false,
    };
    let initial_created = match &initial_destination.container.kind {
        super::types::PreparedContainerKind::Drawer { created, .. }
        | super::types::PreparedContainerKind::Section { created, .. } => {
            *created
        }
        _ => false,
    };
    if staged_created || initial_created {
        match &mut final_destination.container.kind {
            super::types::PreparedContainerKind::Drawer { created, .. }
            | super::types::PreparedContainerKind::Section {
                created, ..
            } => {
                *created = true;
            }
            _ => {}
        }
    }
    let fragments: Vec<String> = prepared_payloads
        .iter()
        .map(|payload| payload.contents.clone())
        .collect();
    let (destination_postimage, landing_lines) =
        transplant::insert_fragments_at_byte(
            &final_destination.contents,
            final_destination.container.insertion_offset,
            &fragments,
            final_destination.container.empty_heading,
        )
        .ok_or_else(|| {
            MergeError::new(
                "merge_container_failed",
                "destination insertion offset is not a character boundary",
            )
        })?;
    // Capture exact insertion extents in destination-postimage coordinates
    // using the repaired fragments. Each payload group's lines share its
    // source identity; coincident text stays distinct by range, never by
    // search.
    let mut insertion_origins = Vec::with_capacity(raw_payloads.len());
    for ((raw, landing), prepared) in raw_payloads
        .iter()
        .zip(landing_lines.iter())
        .zip(prepared_payloads.iter())
    {
        let Some(source) = sources.get(raw.source_index) else {
            continue;
        };
        if source.prepared.disposition == SourceDisposition::Closed {
            continue;
        }
        insertion_origins.push(super::types::PayloadInsertion {
            source_path: source.prepared.path.clone(),
            source_block_id: source.prepared.block_id.clone(),
            start_line: *landing,
            line_count: prepared.contents.lines().count().max(1),
        });
    }

    let source_postimages: BTreeMap<PathBuf, String> = source_ranges
        .keys()
        .filter_map(|path| {
            staged
                .notes
                .get(path)
                .map(|contents| (path.clone(), contents.clone()))
        })
        .collect();
    let repair_counts = BTreeMap::from([(
        final_destination.container.path.clone(),
        (
            repaired_destination.link_count + payload_link_count,
            repaired_destination.dependency_metadata_count
                + payload_dependency_count,
        ),
    )]);
    let repaired = transplant::repair_vault_postimages(
        &snapshot.notes,
        &source_postimages,
        &final_destination.container.path,
        destination_postimage,
        &note_index,
        &moved_targets,
        repair_counts,
    );
    let mut warnings = Vec::new();
    if let Some(warning) =
        staged_destination.warning.or(initial_destination.warning)
    {
        warnings.push(warning);
    }
    let sources = sources.into_iter().map(|source| source.prepared).collect();

    let insertion_dest = Some(final_destination.container.path.clone());
    Ok(PreparedMerge {
        container: Some(final_destination.container),
        sources,
        payloads: prepared_payloads,
        postimages: repaired.postimages,
        renamed_block_ids,
        repaired_files: repaired.repaired_files,
        read_paths: snapshot.notes.keys().cloned().collect(),
        warnings,
        insertion_dest,
        insertion_origins,
    })
}

fn source_payload(
    contents: &str,
    source: &LocatedTask,
    context: &MergeContext<'_>,
) -> Result<(Vec<LineRange>, Vec<(String, String)>, usize), MergeError> {
    let lines = capture::line_spans(contents);
    let tasks = note_tasks::scan(contents, context.task_settings);
    let open_nested = tasks
        .tasks()
        .iter()
        .filter(|task| task.line_index > source.task.line_index)
        .filter(|task| task.status_type.is_open())
        .filter(|task| {
            lines
                .get(task.line_index)
                .is_some_and(|line| line.end <= source.task.block_end)
        })
        .count();
    if open_nested > 0 {
        return Err(MergeError::new(
            "merge_source_open_subtasks",
            format!(
                "“{}” has {open_nested} open subtasks; move or close ^{} first",
                source.task.description,
                source.task.block_id.as_deref().unwrap_or("?")
            ),
        ));
    }

    let children = task_children::classify_direct_children(
        contents,
        &source.task,
        context.task_settings,
    );
    let end_line = lines
        .iter()
        .position(|line| line.end > source.task.block_end)
        .unwrap_or(lines.len());
    let source_unit = capture::first_child_indentation(
        &lines,
        source.task.line_index,
        source.task.block_end,
        &source.task.indentation,
    )
    .and_then(|indent| {
        indent
            .strip_prefix(&source.task.indentation)
            .map(str::to_string)
    })
    .filter(|indent| !indent.is_empty())
    .unwrap_or_else(|| {
        capture::dominant_indent_unit(&lines)
            .unwrap_or("\t")
            .to_string()
    });
    // Direct list roots define the boundaries of managed child blocks, but
    // authored bytes also include continuations before the first root and
    // paragraphs, tables, or fences with no wrapper bullet. Move the whole
    // task body, cutting out only direct managed roots and their nested bodies.
    let body_start = source.task.line_index + 1;
    let mut ranges = Vec::new();
    let mut cursor = body_start;
    for (offset, child) in children.iter().enumerate() {
        if !child.kind.is_managed() {
            continue;
        }
        let end = children
            .get(offset + 1)
            .map(|next| next.line_index)
            .unwrap_or(end_line);
        if cursor < child.line_index {
            ranges.push(LineRange {
                start: cursor,
                end: child.line_index,
            });
        }
        cursor = cursor.max(end);
    }
    if cursor < end_line {
        ranges.push(LineRange {
            start: cursor,
            end: end_line,
        });
    }

    let mut fragments = Vec::new();
    for range in &ranges {
        let fragment =
            transplant::cut_line_range(contents, range).ok_or_else(|| {
                MergeError::new(
                    "merge_source_structure",
                    format!(
                        "could not read child block of ^{}",
                        source.task.block_id.as_deref().unwrap_or("?")
                    ),
                )
            })?;
        fragments.push((fragment, source_unit.clone()));
    }
    let authored_roots = children
        .iter()
        .filter(|child| !child.kind.is_managed())
        .count();
    let moved_notes = if authored_roots > 0 {
        authored_roots
    } else if fragments
        .iter()
        .any(|(fragment, _)| !fragment.trim().is_empty())
    {
        1
    } else {
        0
    };
    Ok((ranges, fragments, moved_notes))
}

fn build_payloads(
    sources: &[SourceWork],
    destination: &ResolvedDestination,
) -> Vec<RawPayload> {
    let ending = transplant::document_line_ending(&destination.contents);
    let mut payloads = Vec::new();
    for (source_index, source) in sources.iter().enumerate() {
        if source.prepared.disposition == SourceDisposition::Closed {
            continue;
        }
        let mut group = format!(
            "{}- {}{ending}",
            destination.title_indent, source.prepared.title
        );
        for (fragment, source_unit) in &source.child_fragments {
            let child_indent = first_fragment_indent(fragment)
                .unwrap_or_else(|| source_unit.clone());
            let rebased = transplant::rebase_fragment(
                fragment,
                &child_indent,
                &destination.child_indent,
                source_unit,
                &destination.child_unit,
                true,
            );
            if !group.ends_with('\n') {
                group.push_str(ending);
            }
            group.push_str(&rebased);
        }
        payloads.push(RawPayload {
            source_index,
            source_path: source.prepared.path.clone(),
            contents: group,
        });
    }
    payloads
}

fn first_fragment_indent(fragment: &str) -> Option<String> {
    fragment
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(capture::leading_whitespace)
        .map(str::to_string)
}

fn source_id_maps(
    payloads: &[RawPayload],
    renamed: &[String],
) -> Result<BTreeMap<PathBuf, BTreeMap<String, String>>, MergeError> {
    let mut maps = BTreeMap::<PathBuf, BTreeMap<String, String>>::new();
    for (payload, new_fragment) in payloads.iter().zip(renamed.iter()) {
        let old_ids = transplant::live_block_ids(&payload.contents);
        let new_ids = transplant::live_block_ids(new_fragment);
        if old_ids.len() != new_ids.len() {
            return Err(MergeError::new(
                "merge_source_ambiguous_anchor",
                "descendant anchor repair changed the number of anchors",
            ));
        }
        let map = maps.entry(payload.source_path.clone()).or_default();
        for (old, new) in old_ids.into_iter().zip(new_ids) {
            if let Some(previous) = map.insert(old.clone(), new.clone())
                && previous != new
            {
                return Err(MergeError::new(
                    "merge_source_ambiguous_anchor",
                    format!(
                        "^{} is repeated in source note {}",
                        old,
                        payload.source_path.display()
                    ),
                ));
            }
        }
    }
    Ok(maps)
}

fn moved_targets(
    payloads: &[RawPayload],
    renamed: &[String],
    destination: &Path,
) -> Result<MovedBlockTargets, MergeError> {
    let destination_target =
        vault_relative_link_target(destination, "merge destination").map_err(
            |error| MergeError::new("merge_invalid_path", error.to_string()),
        )?;
    let mut targets = MovedBlockTargets::new();
    for (payload, new_fragment) in payloads.iter().zip(renamed.iter()) {
        let old_ids = transplant::live_block_ids(&payload.contents);
        let new_ids = transplant::live_block_ids(new_fragment);
        for (old_id, new_id) in old_ids.into_iter().zip(new_ids) {
            let old_dependency_id =
                task_dependencies::dependency_id(&payload.source_path, &old_id)
                    .map_err(|error| {
                        MergeError::new("merge_invalid_path", error.to_string())
                    })?;
            let new_dependency_id =
                task_dependencies::dependency_id(destination, &new_id)
                    .map_err(|error| {
                        MergeError::new("merge_invalid_path", error.to_string())
                    })?;
            targets.insert(
                (payload.source_path.clone(), old_id),
                MovedBlockTarget {
                    archive_target: destination_target.clone(),
                    block_id: new_id,
                    old_dependency_id,
                    new_dependency_id,
                },
            );
        }
    }
    Ok(targets)
}

fn validate_target_and_source_ranges(
    snapshot: &VaultSnapshot,
    sources: &[SourceWork],
    target_task: Option<&LocatedTask>,
) -> Result<(), MergeError> {
    if let Some(target_task) = target_task {
        if sources.iter().any(|source| {
            source.prepared.path == target_task.path
                && source.prepared.block_id
                    == target_task.task.block_id.as_deref().unwrap_or_default()
        }) {
            return Err(MergeError::new(
                "merge_target_is_source",
                "the destination task is also a source",
            ));
        }
        if sources.iter().any(|source| {
            if source.prepared.path != target_task.path {
                return false;
            }
            let Some(contents) = snapshot.notes.get(&target_task.path) else {
                return false;
            };
            let lines = capture::line_spans(contents);
            let target_start =
                line_start_offset(&lines, target_task.task.line_index);
            target_start > source.start_offset
                && target_start < source.located.task.block_end
        }) {
            return Err(MergeError::new(
                "merge_target_inside_source",
                "the destination task is nested under a source task",
            ));
        }
    }

    for (index, left) in sources.iter().enumerate() {
        for right in &sources[index + 1..] {
            if left.prepared.path != right.prepared.path {
                continue;
            }
            let left_range = left.start_offset..left.located.task.block_end;
            let right_range = right.start_offset..right.located.task.block_end;
            if left_range.start < right_range.end
                && right_range.start < left_range.end
            {
                return Err(MergeError::new(
                    "merge_source_overlap",
                    format!(
                        "source task blocks ^{} and ^{} overlap",
                        left.prepared.block_id, right.prepared.block_id
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn target_task(
    snapshot: &VaultSnapshot,
    container: &super::types::MergeContainer,
    context: &MergeContext<'_>,
) -> Result<Option<LocatedTask>, MergeError> {
    match container {
        super::types::MergeContainer::Task(locator)
        | super::types::MergeContainer::Drawer(locator, _) => {
            containers::resolve_task(snapshot, locator, context).map(Some)
        }
        super::types::MergeContainer::Section { .. } => Ok(None),
    }
}

fn note_contents<'a>(
    snapshot: &'a VaultSnapshot,
    path: &Path,
) -> Result<&'a str, MergeError> {
    snapshot.notes.get(path).map(String::as_str).ok_or_else(|| {
        MergeError::new(
            "task_not_found",
            format!("no note matches `{}`", path.display()),
        )
    })
}

fn line_start_offset(lines: &[capture::LineSpan<'_>], index: usize) -> usize {
    if index == 0 {
        0
    } else {
        lines.get(index - 1).map(|line| line.end).unwrap_or(0)
    }
}

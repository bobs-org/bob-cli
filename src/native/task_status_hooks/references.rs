//! Task reference resolution and desired-status computation.
use super::*;

pub(super) fn archive_reference_catalog<'a, I>(
    vault: &Path,
    settings: &TasksSettings,
    references: I,
    inputs: &mut Vec<InputSnapshot>,
) -> Result<ArchiveReferenceCatalog, SyncError>
where
    I: IntoIterator<Item = &'a RawReference>,
{
    let mut catalog = ArchiveReferenceCatalog::default();
    let targets = references
        .into_iter()
        .filter_map(|reference| {
            explicit_archive_reference_path(reference.target.trim())
        })
        .collect::<BTreeSet<_>>();
    let canonical_vault = vault.canonicalize().ok();

    for relative_path in targets {
        let display = display_path(&relative_path);
        let path = vault.join(&relative_path);
        if !path.exists() {
            catalog.load_failures.insert(
                relative_path.clone(),
                format!("archive target {display} does not exist"),
            );
            push_unique_input(
                inputs,
                InputSnapshot::missing(&path, InputKind::Archive),
            );
            continue;
        }
        if !path.is_file() {
            catalog.load_failures.insert(
                relative_path.clone(),
                format!("archive target {display} is not a file"),
            );
            continue;
        }
        if let Some(canonical_vault) = &canonical_vault {
            match path.canonicalize() {
                Ok(canonical_path)
                    if canonical_path.starts_with(canonical_vault) => {}
                Ok(_) => {
                    catalog.load_failures.insert(
                        relative_path.clone(),
                        format!(
                            "archive target {display} is outside the vault root"
                        ),
                    );
                    continue;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    catalog.load_failures.insert(
                        relative_path.clone(),
                        format!("archive target {display} does not exist"),
                    );
                    continue;
                }
                Err(error) => {
                    return Err(SyncError::io(
                        "inspect archive note",
                        &path,
                        error,
                    ));
                }
            }
        }

        let snapshot = match capture_optional(&path, InputKind::Archive) {
            Ok(snapshot) => snapshot,
            Err(CaptureError::Unsupported { .. }) => {
                catalog.load_failures.insert(
                    relative_path.clone(),
                    format!("archive target {display} is not a file"),
                );
                continue;
            }
            Err(error) => {
                return Err(capture_to_sync("read archive note", error));
            }
        };
        if snapshot.is_missing() {
            catalog.load_failures.insert(
                relative_path.clone(),
                format!("archive target {display} does not exist"),
            );
            push_unique_input(inputs, snapshot);
            continue;
        }
        let contents = match snapshot.utf8_contents() {
            Ok(Some(contents)) => contents,
            Ok(None) => {
                catalog.load_failures.insert(
                    relative_path.clone(),
                    format!("archive target {display} does not exist"),
                );
                push_unique_input(inputs, snapshot);
                continue;
            }
            Err(error) => {
                return Err(capture_to_sync("read archive note", error));
            }
        };
        push_unique_input(inputs, snapshot);
        catalog.note_paths.insert(relative_path.clone());
        for task in parse_tasks(&contents, settings) {
            if let Some(block_id) = task.block_id {
                catalog
                    .task_blocks
                    .entry((relative_path.clone(), block_id))
                    .or_insert_with(Vec::new)
                    .push(task.status);
            }
        }
    }

    Ok(catalog)
}

pub(super) fn explicit_archive_reference_path(target: &str) -> Option<PathBuf> {
    let path = target_to_markdown_path(target)?;
    let mut components = path.components();
    let Some(Component::Normal(first)) = components.next() else {
        return None;
    };
    if first != OsStr::new("done") {
        return None;
    }
    let Some(Component::Normal(_)) = components.next() else {
        return None;
    };
    Some(path)
}

impl<'a> TaskReferenceResolver<'a> {
    pub(super) fn resolve_path(
        &self,
        current_path: Option<&Path>,
        target: &str,
    ) -> Result<PathBuf, ResolvePathError> {
        if let Some(path) = self.note_index.resolve(current_path, target) {
            return Ok(path);
        }
        let Some(path) = explicit_archive_reference_path(target) else {
            return Err(ResolvePathError::Unresolved);
        };
        if self.archive_catalog.note_paths.contains(&path) {
            return Ok(path);
        }
        if let Some(reason) = self.archive_catalog.load_failures.get(&path) {
            return Err(ResolvePathError::ArchiveUnavailable(reason.clone()));
        }
        Err(ResolvePathError::Unresolved)
    }

    pub(super) fn statuses(
        &self,
        identity: &(PathBuf, String),
    ) -> Option<&Vec<char>> {
        self.task_blocks
            .get(identity)
            .or_else(|| self.archive_catalog.task_blocks.get(identity))
    }
}

impl ReferenceContext {
    pub(super) fn unresolved_reason(self, error: ResolvePathError) -> String {
        match (self, error) {
            (Self::CurrentDaily, ResolvePathError::Unresolved) => {
                "note target did not resolve uniquely".to_string()
            }
            (Self::PreviousDaily, ResolvePathError::Unresolved) => {
                "previous daily note target did not resolve uniquely"
                    .to_string()
            }
            (
                Self::CurrentDaily,
                ResolvePathError::ArchiveUnavailable(reason),
            ) => reason,
            (
                Self::PreviousDaily,
                ResolvePathError::ArchiveUnavailable(reason),
            ) => {
                format!("previous daily note {reason}")
            }
        }
    }

    pub(super) fn missing_block_reason(self, path: &Path) -> String {
        match self {
            Self::CurrentDaily => {
                format!("{} has no matching task block", display_path(path))
            }
            Self::PreviousDaily => format!(
                "previous daily note target {} has no matching task block",
                display_path(path)
            ),
        }
    }

    pub(super) fn duplicate_block_reason(
        self,
        path: &Path,
        statuses: &[char],
        settings: &TasksSettings,
    ) -> String {
        match self {
            Self::PreviousDaily => format!(
                "previous daily note target {} has {} tasks with this block id; all were matched",
                display_path(path),
                statuses.len()
            ),
            Self::CurrentDaily => current_duplicate_block_reason(
                path, statuses, settings,
            ),
        }
    }
}

pub(super) fn current_duplicate_block_reason(
    path: &Path,
    statuses: &[char],
    settings: &TasksSettings,
) -> String {
    let has_done = statuses
        .iter()
        .any(|status| is_done_status(*status, &settings.done_statuses));
    let has_not_done = statuses
        .iter()
        .any(|status| !is_done_status(*status, &settings.done_statuses));
    let has_canceled = statuses
        .iter()
        .any(|status| is_canceled_status(*status, &settings.status_types));
    let has_not_canceled = statuses
        .iter()
        .any(|status| !is_canceled_status(*status, &settings.status_types));
    if has_done && has_not_done && has_canceled {
        format!(
            "{} has {} tasks with conflicting statuses; completed-link normalization and canceled-reference list-item removal were skipped",
            display_path(path),
            statuses.len()
        )
    } else if has_done && has_not_done {
        format!(
            "{} has {} tasks with conflicting statuses; completed-link normalization was skipped",
            display_path(path),
            statuses.len()
        )
    } else if has_canceled && has_not_canceled {
        format!(
            "{} has {} tasks with conflicting statuses; canceled-reference list-item removal was skipped",
            display_path(path),
            statuses.len()
        )
    } else {
        format!(
            "{} has {} tasks with this block id; all were matched",
            display_path(path),
            statuses.len()
        )
    }
}

pub(super) fn resolve_task_reference(
    reference: &RawReference,
    source_path: Option<&Path>,
    resolver: &TaskReferenceResolver<'_>,
    context: ReferenceContext,
    settings: &TasksSettings,
    unresolved: &mut Vec<UnresolvedReference>,
) -> Option<ResolvedReference> {
    let path = match resolver.resolve_path(source_path, reference.target.trim())
    {
        Ok(path) => path,
        Err(error) => {
            unresolved.push(UnresolvedReference {
                target: reference.target.clone(),
                block_id: reference.block_id.clone(),
                reason: context.unresolved_reason(error),
            });
            return None;
        }
    };
    let identity = (path.clone(), reference.block_id.clone());
    let Some(statuses) = resolver.statuses(&identity).cloned() else {
        unresolved.push(UnresolvedReference {
            target: reference.target.clone(),
            block_id: reference.block_id.clone(),
            reason: context.missing_block_reason(&path),
        });
        return None;
    };
    if statuses.len() > 1 {
        unresolved.push(UnresolvedReference {
            target: reference.target.clone(),
            block_id: reference.block_id.clone(),
            reason: context.duplicate_block_reason(&path, &statuses, settings),
        });
    }
    Some(ResolvedReference { path, statuses })
}

/// Promotion edges (`docs/task-dependencies.md` §5): the resolved
/// Depends-On line links plus the R8 legacy children whose target id is
/// managed by the dependent's field. Sole embeds that are not field
/// managed (for example `#^ref` reading embeds), links into `done/`,
/// and non-task targets are never edges.
pub(super) fn dependency_edges(
    files: &[FileScan],
    note_index: &NoteIndex,
    task_blocks: &BTreeMap<(PathBuf, String), Vec<char>>,
    unresolved: &mut Vec<UnresolvedReference>,
) -> BTreeMap<(PathBuf, String), BTreeSet<(PathBuf, String)>> {
    let mut edges: BTreeMap<(PathBuf, String), BTreeSet<(PathBuf, String)>> =
        BTreeMap::new();
    let mut task_ids: BTreeMap<(PathBuf, String), Option<String>> =
        BTreeMap::new();
    for file in files {
        for task in &file.tasks {
            if let Some(block_id) = &task.block_id {
                task_ids.insert(
                    (file.relative_path.clone(), block_id.clone()),
                    task.task_id.clone(),
                );
            }
        }
    }
    for file in files {
        if !file.tasks.iter().any(|task| task.block_id.is_some()) {
            continue;
        }
        let lines = logical_lines(&file.contents);
        let fenced = fenced_lines(&lines, 0..lines.len());
        for task in &file.tasks {
            let Some(source_block_id) = &task.block_id else {
                continue;
            };
            let source = (file.relative_path.clone(), source_block_id.clone());
            let source_indent =
                leading_indentation_width(lines[task.line_index]);
            let dependency_child =
                task_dependencies::parse::dependency_child_of(
                    &lines,
                    &fenced,
                    task.line_index,
                );
            if let Some(child) = &dependency_child
                && let task_dependencies::DependencyLine::Accepted {
                    links, ..
                } = &child.parsed
            {
                for link in links {
                    if explicit_archive_reference_path(link.target.trim())
                        .is_some()
                    {
                        continue;
                    }
                    let Some(target_path) = note_index
                        .resolve(Some(&file.relative_path), link.target.trim())
                    else {
                        push_unresolved(
                            unresolved,
                            &file.relative_path,
                            task.line_index,
                            link.target.clone(),
                            link.block_id.clone(),
                            None,
                        );
                        continue;
                    };
                    if task_dependencies::is_archive_path(&target_path) {
                        continue;
                    }
                    let target = (target_path.clone(), link.block_id.clone());
                    if !task_blocks.contains_key(&target) {
                        push_unresolved(
                            unresolved,
                            &file.relative_path,
                            task.line_index,
                            link.target.clone(),
                            link.block_id.clone(),
                            Some(&target_path),
                        );
                        continue;
                    }
                    edges.entry(source.clone()).or_default().insert(target);
                }
            }
            for line_index in task.line_index + 1..lines.len() {
                if dependency_child
                    .as_ref()
                    .is_some_and(|child| child.line_index == line_index)
                {
                    continue;
                }
                let line = lines[line_index];
                if line.trim().is_empty() {
                    continue;
                }
                if fenced.contains(&line_index) {
                    continue;
                }
                let indentation = leading_indentation_width(line);
                if indentation <= source_indent {
                    break;
                }
                if nearest_parent_list_item(&lines, line_index)
                    != Some(task.line_index)
                {
                    continue;
                }
                let Some(legacy) =
                    task_dependencies::legacy_child_reference(line)
                else {
                    continue;
                };
                if explicit_archive_reference_path(legacy.target.trim())
                    .is_some()
                {
                    continue;
                }
                let Some(target_path) = note_index
                    .resolve(Some(&file.relative_path), legacy.target.trim())
                else {
                    push_unresolved(
                        unresolved,
                        &file.relative_path,
                        task.line_index,
                        legacy.target,
                        legacy.block_id,
                        None,
                    );
                    continue;
                };
                if task_dependencies::is_archive_path(&target_path) {
                    continue;
                }
                let target = (target_path.clone(), legacy.block_id.clone());
                if !task_blocks.contains_key(&target) {
                    push_unresolved(
                        unresolved,
                        &file.relative_path,
                        task.line_index,
                        legacy.target,
                        legacy.block_id,
                        Some(&target_path),
                    );
                    continue;
                }
                if !legacy_child_covered(
                    task,
                    &file.relative_path,
                    &target_path,
                    &target.1,
                    &task_ids,
                ) {
                    continue;
                }
                edges.entry(source.clone()).or_default().insert(target);
            }
        }
    }
    edges
}

fn push_unresolved(
    unresolved: &mut Vec<UnresolvedReference>,
    source_path: &Path,
    task_line_index: usize,
    target: String,
    block_id: String,
    resolved_path: Option<&Path>,
) {
    let reason = match resolved_path {
        None => format!(
            "dependency from {}:{} did not resolve uniquely",
            display_path(source_path),
            task_line_index + 1
        ),
        Some(target_path) => format!(
            "dependency from {}:{} resolved to {}, which has no matching task block",
            display_path(source_path),
            task_line_index + 1,
            display_path(target_path)
        ),
    };
    unresolved.push(UnresolvedReference {
        target,
        block_id,
        reason,
    });
}

/// R8 field gate: the legacy child's resolved target id (its `[id::]`,
/// canonical id, or same-note bare block id) is in the dependent's
/// field.
fn legacy_child_covered(
    task: &TaskLine,
    source_path: &Path,
    target_path: &Path,
    block_id: &str,
    task_ids: &BTreeMap<(PathBuf, String), Option<String>>,
) -> bool {
    if task.depends_on.is_empty() {
        return false;
    }
    if let Some(Some(task_id)) =
        task_ids.get(&(target_path.to_path_buf(), block_id.to_string()))
    {
        if task
            .depends_on
            .iter()
            .any(|dependency| dependency == task_id)
        {
            return true;
        }
    }
    if let Ok(canonical) =
        task_dependencies::dependency_id(target_path, block_id)
        && task
            .depends_on
            .iter()
            .any(|dependency| dependency == &canonical)
    {
        return true;
    }
    target_path == source_path
        && task
            .depends_on
            .iter()
            .any(|dependency| dependency == block_id)
}

pub(super) fn resolve_recent_references(
    references: &BTreeSet<RawReference>,
    source_path: Option<&Path>,
    resolver: &TaskReferenceResolver<'_>,
    settings: &TasksSettings,
    unresolved: &mut Vec<UnresolvedReference>,
) -> BTreeSet<(PathBuf, String)> {
    let mut resolved_identities = BTreeSet::new();
    for reference in references {
        if let Some(resolved) = resolve_task_reference(
            reference,
            source_path,
            resolver,
            ReferenceContext::PreviousDaily,
            settings,
            unresolved,
        ) {
            resolved_identities
                .insert((resolved.path, reference.block_id.clone()));
        }
    }
    resolved_identities
}

pub(super) fn desired_statuses(
    direct: &BTreeSet<(PathBuf, String)>,
    edges: &BTreeMap<(PathBuf, String), BTreeSet<(PathBuf, String)>>,
    task_blocks: &BTreeMap<(PathBuf, String), Vec<char>>,
) -> BTreeMap<(PathBuf, String), RankedStatus> {
    let mut desired = BTreeMap::new();
    let mut queue = direct.iter().cloned().collect::<VecDeque<_>>();
    for identity in direct {
        desired.insert(
            identity.clone(),
            strongest_current_status(
                task_blocks.get(identity).map(Vec::as_slice),
            )
            .unwrap_or(RankedStatus::Next)
            .max(RankedStatus::Next),
        );
    }
    while let Some(source) = queue.pop_front() {
        let source_status = desired[&source];
        let Some(targets) = edges.get(&source) else {
            continue;
        };
        for target in targets {
            let target_status = strongest_current_status(
                task_blocks.get(target).map(Vec::as_slice),
            )
            .unwrap_or(source_status)
            .max(source_status);
            let should_update = desired
                .get(target)
                .is_none_or(|current| *current < target_status);
            if should_update {
                desired.insert(target.clone(), target_status);
                queue.push_back(target.clone());
            }
        }
    }
    desired
}

pub(super) fn strongest_current_status(
    statuses: Option<&[char]>,
) -> Option<RankedStatus> {
    statuses
        .into_iter()
        .flatten()
        .filter_map(|status| RankedStatus::from_checkbox(*status))
        .max()
}

pub(crate) fn leading_indentation_width(line: &str) -> usize {
    line.bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .fold(0, |width, byte| {
            if byte == b'\t' {
                width + (4 - width % 4)
            } else {
                width + 1
            }
        })
}

pub(crate) fn nearest_parent_list_item(
    lines: &[&str],
    child_line: usize,
) -> Option<usize> {
    let child_indent = leading_indentation_width(lines.get(child_line)?);
    for line_index in (0..child_line).rev() {
        let line = lines[line_index];
        if line.trim().is_empty()
            || leading_indentation_width(line) >= child_indent
        {
            continue;
        }
        let byte_indent = line
            .bytes()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .count();
        if after_list_marker(line, byte_indent).is_some() {
            return Some(line_index);
        }
    }
    None
}

pub(super) fn change_item(
    file: &FileScan,
    task: &TaskLine,
    dependency: bool,
) -> ChangeItem {
    ChangeItem {
        path: display_path(&file.relative_path),
        line_number: task.line_index + 1,
        block_id: task.block_id.clone().unwrap_or_default(),
        description: task.description.clone(),
        dependency,
    }
}

pub(super) fn dependency_status_change(
    file: &FileScan,
    task: &TaskLine,
    to: char,
    state: &TaskDependencyState,
    future_scheduled_date: Option<NaiveDate>,
) -> DependencyStatusChange {
    DependencyStatusChange {
        path: display_path(&file.relative_path),
        line_number: task.line_index + 1,
        block_id: task.block_id.clone().unwrap_or_default(),
        description: task.description.clone(),
        from: task.status,
        to,
        open_dependency_ids: state.open_dependency_ids.clone(),
        unresolved_dependency_ids: state.unresolved_dependency_ids.clone(),
        future_scheduled_date: future_scheduled_date
            .map(|scheduled| scheduled.to_string()),
    }
}

pub(super) fn display_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::{Component, Path, PathBuf},
};

use serde::Serialize;

#[cfg(test)]
use std::{fs, io};

use super::super::{
    capture_dependency_tasks::has_prj_is_blocked_tag,
    collect_done::{
        self, block_id_occurrences_in_text, has_task_tag,
        is_simple_markdown_destination, leading_indent_len,
        next_available_block_id, repair_dependency_metadata,
        repair_links_in_note_with, repair_markdown_destination,
        repair_wiki_link_inner, split_block_fragment, split_line_ending,
        task_block_end, trailing_block_id_in_line, vault_relative_link_target,
        MovedBlockTarget, MovedBlockTargets,
    },
    markdown,
    note_tasks::clean_description,
    projects::{
        frontmatter_is_area, frontmatter_is_project, frontmatter_value,
        parse_frontmatter, trim_yaml_scalar, ProjectStatus,
    },
    task_complete::is_inbox_note,
    task_dependencies::{
        dependency_id, inline_code_spans, is_archive_path, parse,
        DependencyLine,
    },
    task_fields::inline_fields,
    task_status_groups,
    vault_links::{self, NoteIndex},
};

#[cfg(test)]
use super::super::collect_done::link_repair_markdown_files;

const OPEN_CHECKBOXES: [char; 4] = [' ', '*', '/', '?'];
const GLOBAL_FILTER: &str = "#task";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VaultSnapshot {
    pub(crate) notes: BTreeMap<PathBuf, String>,
}

impl VaultSnapshot {
    pub(crate) fn from_notes<I>(notes: I) -> Self
    where
        I: IntoIterator<Item = (PathBuf, String)>,
    {
        Self {
            notes: notes.into_iter().collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MoveRequest {
    pub(crate) source: String,
    pub(crate) dest: String,
    pub(crate) selectors: Vec<MoveSelector>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MoveSelector {
    Block(String),
    Match(String),
}

impl fmt::Display for MoveSelector {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Block(id) => write!(formatter, "block id {id}"),
            Self::Match(text) => write!(formatter, "match {text:?}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MovePlan {
    pub(crate) postimages: BTreeMap<PathBuf, String>,
    pub(crate) moves: Vec<MoveRecord>,
    pub(crate) renamed_block_ids: Vec<RenameRecord>,
    pub(crate) repaired_files: Vec<RepairedFile>,
    pub(crate) needs_reconcile: bool,
}

/// A routable area or open project reported to capture clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TaskMoveDestination {
    pub(crate) relative_path: String,
    pub(crate) route: String,
    pub(crate) label: String,
    pub(crate) kind: &'static str,
    pub(crate) status: Option<String>,
    pub(crate) parent: Option<String>,
    pub(crate) blocked: bool,
}

/// Enumerate the same destination kinds accepted by `plan_task_move`,
/// excluding inbox notes which must never be offered as a route target.
pub(crate) fn list_task_move_destinations(
    snapshot: &VaultSnapshot,
) -> Vec<TaskMoveDestination> {
    let note_index = NoteIndex::from_paths(snapshot.notes.keys().cloned());
    snapshot
        .notes
        .iter()
        .filter_map(|(path, contents)| {
            if is_route_destination_excluded(path)
                || is_inbox_note(path, contents, &note_index)
            {
                return None;
            }
            let kind = classify_dest(path, contents).ok()?;
            let frontmatter = parse_frontmatter(contents)?;
            let status = (kind == DestKind::Project).then(|| {
                ProjectStatus::parse(frontmatter_value(&frontmatter, "status"))
                    .label()
                    .to_string()
            });
            let parent =
                frontmatter_value(&frontmatter, "parent").and_then(|value| {
                    resolve_parent_label(value, path, &note_index)
                });
            let route =
                path.with_extension("").to_string_lossy().replace('\\', "/");
            let label = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_string();
            Some(TaskMoveDestination {
                relative_path: path.to_string_lossy().replace('\\', "/"),
                route,
                label,
                kind: match kind {
                    DestKind::Area => "area",
                    DestKind::Project => "project",
                },
                status,
                parent,
                blocked: dest_prj_is_blocked(contents),
            })
        })
        .collect()
}

fn is_route_destination_excluded(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component,
            Component::Normal(name)
                if name == "done" || name == "_templates" || name == "_conflicts"
        )
    })
}

fn resolve_parent_label(
    raw: &str,
    path: &Path,
    note_index: &NoteIndex,
) -> Option<String> {
    let value = trim_yaml_scalar(raw.trim());
    let value = value
        .trim_matches(['[', ']'])
        .split(',')
        .next()?
        .trim()
        .trim_matches(['"', '\'']);
    let target = value
        .strip_prefix("[[")
        .and_then(|value| value.strip_suffix("]]"))
        .unwrap_or(value)
        .split('|')
        .next()?
        .split('#')
        .next()?
        .trim();
    let resolved = note_index.resolve(Some(path), target)?;
    resolved.file_stem()?.to_str().map(str::to_string)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MoveRecord {
    pub(crate) source_path: PathBuf,
    pub(crate) dest_path: PathBuf,
    pub(crate) source_line: usize,
    pub(crate) title: String,
    pub(crate) old_block_id: Option<String>,
    pub(crate) new_block_id: Option<String>,
    pub(crate) old_dependency_id: Option<String>,
    pub(crate) new_dependency_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenameRecord {
    pub(crate) path: PathBuf,
    pub(crate) old_block_id: String,
    pub(crate) new_block_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepairedFile {
    pub(crate) path: PathBuf,
    pub(crate) link_count: usize,
    pub(crate) dependency_token_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskCandidate {
    pub(crate) line: usize,
    pub(crate) title: String,
    pub(crate) block_id: Option<String>,
    pub(crate) status: char,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlanError {
    NoSelectors,
    InvalidPath {
        query: String,
    },
    MissingNote {
        query: String,
    },
    AmbiguousNote {
        query: String,
        candidates: Vec<PathBuf>,
    },
    SameSourceAndDest {
        path: PathBuf,
    },
    ArchivePath {
        path: PathBuf,
        role: &'static str,
    },
    DestKind {
        path: PathBuf,
        detail: String,
    },
    ProjectWithoutTasks {
        path: PathBuf,
    },
    SelectorZero {
        selector: MoveSelector,
        candidates: Vec<TaskCandidate>,
    },
    SelectorAmbiguous {
        selector: MoveSelector,
        candidates: Vec<TaskCandidate>,
    },
    ClosedTask {
        candidate: TaskCandidate,
    },
    NestedTask {
        candidate: TaskCandidate,
    },
    ProjectTask {
        candidate: TaskCandidate,
    },
    UnsupportedCheckbox {
        candidate: TaskCandidate,
    },
    NotATask {
        line: usize,
        excerpt: String,
        block_id: String,
    },
    DependsOnDestPrj {
        candidate: TaskCandidate,
        dest: PathBuf,
    },
    DestBlocked {
        candidate: TaskCandidate,
        dest: PathBuf,
    },
    AmbiguousIdentity {
        path: PathBuf,
        block_id: String,
    },
    NonCanonicalId {
        candidate: TaskCandidate,
        found: String,
        expected: String,
    },
    EncodedIdentityCollision {
        token: String,
        first: PathBuf,
        first_block: String,
        second: PathBuf,
        second_block: String,
    },
    UnrepresentableIdentity {
        path: PathBuf,
        block_id: String,
        detail: String,
    },
}

impl fmt::Display for PlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSelectors => {
                write!(formatter, "at least one -b/--block or -m/--match selector is required")
            }
            Self::InvalidPath { query } => {
                write!(
                    formatter,
                    "note path {query:?} is absolute, escaping, or otherwise not vault-relative"
                )
            }
            Self::MissingNote { query } => {
                write!(formatter, "note {query:?} was not found in the vault")
            }
            Self::AmbiguousNote { query, candidates } => {
                write!(formatter, "note {query:?} is ambiguous:")?;
                for path in candidates {
                    write!(formatter, " {}", path.display())?;
                }
                Ok(())
            }
            Self::SameSourceAndDest { path } => {
                write!(
                    formatter,
                    "source and destination resolve to the same note {}",
                    path.display()
                )
            }
            Self::ArchivePath { path, role } => {
                write!(
                    formatter,
                    "{role} {} is under done/ and cannot be used",
                    path.display()
                )
            }
            Self::DestKind { path, detail } => {
                write!(formatter, "{}: {detail}", path.display())
            }
            Self::ProjectWithoutTasks { path } => {
                write!(
                    formatter,
                    "project {} has no ## Tasks section",
                    path.display()
                )
            }
            Self::SelectorZero {
                selector,
                candidates,
            } => {
                write!(formatter, "{selector} matched no movable task")?;
                write_candidates(formatter, candidates)
            }
            Self::SelectorAmbiguous {
                selector,
                candidates,
            } => {
                write!(formatter, "{selector} matched multiple tasks")?;
                write_candidates(formatter, candidates)
            }
            Self::ClosedTask { candidate } => {
                write!(
                    formatter,
                    "refusing closed task {}",
                    format_candidate(candidate)
                )
            }
            Self::NestedTask { candidate } => {
                write!(
                    formatter,
                    "refusing nested task {}",
                    format_candidate(candidate)
                )
            }
            Self::ProjectTask { candidate } => {
                write!(
                    formatter,
                    "refusing project task {}",
                    format_candidate(candidate)
                )
            }
            Self::UnsupportedCheckbox { candidate } => {
                write!(
                    formatter,
                    "refusing task {} with checkbox [{}]",
                    format_candidate(candidate),
                    candidate.status
                )
            }
            Self::NotATask {
                line,
                excerpt,
                block_id,
            } => {
                write!(
                    formatter,
                    "block id {block_id} on line {line} is not a movable task: {excerpt}"
                )
            }
            Self::DependsOnDestPrj { candidate, dest } => {
                write!(
                    formatter,
                    "task {} depends on {}'s ^prj; drop that link first",
                    format_candidate(candidate),
                    dest.display()
                )
            }
            Self::DestBlocked { candidate, dest } => {
                write!(
                    formatter,
                    "destination {} is blocked; only [?] tasks can move there. Ctrl+Shift+M handles promotion and today's Task Link pruning ({})",
                    dest.display(),
                    format_candidate(candidate)
                )
            }
            Self::AmbiguousIdentity { path, block_id } => {
                write!(
                    formatter,
                    "block id {block_id} is ambiguous in {}",
                    path.display()
                )
            }
            Self::NonCanonicalId {
                candidate,
                found,
                expected,
            } => {
                write!(
                    formatter,
                    "task {} has non-canonical [id:: {found}]; expected {expected}",
                    format_candidate(candidate)
                )
            }
            Self::EncodedIdentityCollision {
                token,
                first,
                first_block,
                second,
                second_block,
            } => {
                write!(
                    formatter,
                    "encoded identity {token} is ambiguous between {}#^{first_block} and {}#^{second_block}",
                    first.display(),
                    second.display()
                )
            }
            Self::UnrepresentableIdentity {
                path,
                block_id,
                detail,
            } => {
                write!(
                    formatter,
                    "cannot encode identity for {}#^{block_id}: {detail}",
                    path.display()
                )
            }
        }
    }
}

fn format_candidate(candidate: &TaskCandidate) -> String {
    let id = candidate
        .block_id
        .as_deref()
        .map(|id| format!(" ^{id}"))
        .unwrap_or_default();
    format!("line {}: {}{id}", candidate.line, candidate.title)
}

fn write_candidates(
    formatter: &mut fmt::Formatter<'_>,
    candidates: &[TaskCandidate],
) -> fmt::Result {
    if candidates.is_empty() {
        return Ok(());
    }
    write!(formatter, ":")?;
    for candidate in candidates {
        write!(formatter, " {}", format_candidate(candidate))?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn load_vault_snapshot(vault: &Path) -> io::Result<VaultSnapshot> {
    let files = link_repair_markdown_files(vault)?;
    let mut notes = BTreeMap::new();
    for path in files {
        let relative = path.strip_prefix(vault).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "note {} is outside vault {}: {error}",
                    path.display(),
                    vault.display()
                ),
            )
        })?;
        let bytes = fs::read(&path)?;
        let contents = String::from_utf8(bytes).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} is not valid UTF-8", relative.display()),
            )
        })?;
        notes.insert(relative.to_path_buf(), contents);
    }
    Ok(VaultSnapshot { notes })
}

pub(crate) fn plan_task_move(
    snapshot: &VaultSnapshot,
    request: &MoveRequest,
) -> Result<MovePlan, PlanError> {
    if request.selectors.is_empty() {
        return Err(PlanError::NoSelectors);
    }

    let note_index = NoteIndex::from_paths(snapshot.notes.keys().cloned());
    let source_path = resolve_note(&note_index, &request.source)?;
    let dest_path = resolve_note(&note_index, &request.dest)?;
    if source_path == dest_path {
        return Err(PlanError::SameSourceAndDest { path: source_path });
    }
    refuse_archive_path(&source_path, "source")?;
    refuse_archive_path(&dest_path, "destination")?;

    let source_contents = snapshot_note(snapshot, &source_path)?;
    let dest_contents = snapshot_note(snapshot, &dest_path)?;
    let dest_kind = classify_dest(&dest_path, dest_contents)?;
    let dest_has_tasks = tasks_heading_index(dest_contents).is_some();
    if dest_kind == DestKind::Project && !dest_has_tasks {
        return Err(PlanError::ProjectWithoutTasks { path: dest_path });
    }

    let source_tasks = scan_task_lines(source_contents);
    let selected =
        select_tasks(source_contents, &source_tasks, &request.selectors)?;
    let dest_blocked = dest_prj_is_blocked(dest_contents);
    let source_lines: Vec<&str> =
        source_contents.split_inclusive('\n').collect();
    let source_logical: Vec<&str> = source_lines
        .iter()
        .map(|line| split_line_ending(line).0)
        .collect();
    let source_fenced =
        markdown::fenced_lines(&source_logical, 0..source_logical.len());

    for task in &selected {
        validate_movable_task(task)?;
        if task_depends_on_dest_prj(
            &source_logical,
            &source_fenced,
            task.line_index,
            &source_path,
            &dest_path,
            &note_index,
        ) {
            return Err(PlanError::DependsOnDestPrj {
                candidate: task.candidate(),
                dest: dest_path.clone(),
            });
        }
        if dest_blocked && task.symbol != '?' {
            return Err(PlanError::DestBlocked {
                candidate: task.candidate(),
                dest: dest_path.clone(),
            });
        }
    }

    let source_id_counts = count_live_block_ids(source_contents);
    let mut blocks = Vec::new();
    for task in &selected {
        let end = task_block_end(&source_lines, task.line_index, 0);
        let fragment = source_lines[task.line_index..end].concat();
        validate_fragment_identities(
            &fragment,
            &source_path,
            &source_id_counts,
            task,
        )?;
        blocks.push(SelectedBlock {
            task: task.clone(),
            end,
            fragment,
        });
    }

    let dest_used: BTreeSet<String> =
        live_block_ids(dest_contents).into_iter().collect();
    let (fragments, renames, id_map) =
        rename_incoming_fragments(&blocks, &dest_path, &dest_used)?;
    validate_mapped_identities(snapshot, &source_path, &dest_path, &id_map)?;

    let dest_target = vault_relative_link_target(&dest_path, "destination")
        .map_err(|error| PlanError::InvalidPath {
            query: format!("{} ({error})", dest_path.display()),
        })?;
    let source_target = vault_relative_link_target(&source_path, "source")
        .map_err(|error| PlanError::InvalidPath {
            query: format!("{} ({error})", source_path.display()),
        })?;

    let mut moved_targets = MovedBlockTargets::new();
    for (old_id, new_id) in &id_map {
        let old_dep = required_dependency_id(&source_path, old_id)?;
        let new_dep = required_dependency_id(&dest_path, new_id)?;
        moved_targets.insert(
            (source_path.clone(), old_id.clone()),
            MovedBlockTarget {
                archive_target: dest_target.clone(),
                block_id: new_id.clone(),
                old_dependency_id: old_dep,
                new_dependency_id: new_dep,
            },
        );
    }

    let moved_old_ids: BTreeSet<String> = id_map.keys().cloned().collect();
    let stayed_source_ids: BTreeSet<String> =
        live_block_ids(&remove_selected_blocks(source_contents, &blocks))
            .into_iter()
            .filter(|id| !moved_old_ids.contains(id))
            .collect();

    let mut fragment_link_count = 0;
    let mut fragment_dep_count = 0;
    let mut repaired_fragments = Vec::new();
    for fragment in &fragments {
        let repaired = repair_moved_fragment(
            fragment,
            &source_path,
            &source_target,
            &note_index,
            &moved_targets,
            &stayed_source_ids,
            &id_map,
        );
        fragment_link_count += repaired.link_count;
        fragment_dep_count += repaired.dependency_count;
        repaired_fragments.push(repaired.contents);
    }

    let dest_existing_repair = repair_links_in_note_with(
        dest_contents,
        &dest_path,
        &note_index,
        &moved_targets,
        true,
    );
    let source_without = remove_selected_blocks(source_contents, &blocks);
    let source_repair = repair_links_in_note_with(
        &source_without,
        &source_path,
        &note_index,
        &moved_targets,
        true,
    );

    let dest_with_insert = insert_into_dest(
        &dest_existing_repair.contents,
        &repaired_fragments,
        dest_kind,
    )?;

    let mut repair_counts: BTreeMap<PathBuf, (usize, usize)> = BTreeMap::new();
    add_repair(
        &mut repair_counts,
        dest_path.clone(),
        dest_existing_repair.link_count + fragment_link_count,
        dest_existing_repair.dependency_metadata_count + fragment_dep_count,
    );
    add_repair(
        &mut repair_counts,
        source_path.clone(),
        source_repair.link_count,
        source_repair.dependency_metadata_count,
    );

    let mut postimages = BTreeMap::new();
    postimages.insert(dest_path.clone(), dest_with_insert);
    postimages.insert(source_path.clone(), source_repair.contents);

    for (path, original) in &snapshot.notes {
        if path == &source_path || path == &dest_path {
            continue;
        }
        let repair = repair_links_in_note_with(
            original,
            path,
            &note_index,
            &moved_targets,
            true,
        );
        if repair.link_count > 0 || repair.dependency_metadata_count > 0 {
            add_repair(
                &mut repair_counts,
                path.clone(),
                repair.link_count,
                repair.dependency_metadata_count,
            );
            postimages.insert(path.clone(), repair.contents);
        }
    }

    postimages.retain(|path, contents| {
        snapshot.notes.get(path).map(String::as_str) != Some(contents.as_str())
    });

    let mut moves = Vec::new();
    let mut carried_block_tag = false;
    for (task, fragment) in selected.iter().zip(fragments.iter()) {
        if has_prj_is_blocked_tag(&task.line) {
            carried_block_tag = true;
        }
        let old_block_id = task.block_id.clone();
        let new_block_id = old_block_id
            .as_ref()
            .map(|id| id_map.get(id).cloned().unwrap_or_else(|| id.clone()));
        let old_dependency_id = old_block_id
            .as_ref()
            .and_then(|id| dependency_id(&source_path, id).ok());
        let new_dependency_id = new_block_id
            .as_ref()
            .and_then(|id| dependency_id(&dest_path, id).ok());
        let _ = fragment;
        moves.push(MoveRecord {
            source_path: source_path.clone(),
            dest_path: dest_path.clone(),
            source_line: task.line_index + 1,
            title: task.title.clone(),
            old_block_id,
            new_block_id,
            old_dependency_id,
            new_dependency_id,
        });
    }

    let repaired_files = repair_counts
        .into_iter()
        .filter(|(_, (links, deps))| *links > 0 || *deps > 0)
        .map(
            |(path, (link_count, dependency_token_count))| RepairedFile {
                path,
                link_count,
                dependency_token_count,
            },
        )
        .collect();

    Ok(MovePlan {
        postimages,
        moves,
        renamed_block_ids: renames,
        repaired_files,
        needs_reconcile: dest_blocked || carried_block_tag,
    })
}

fn snapshot_note<'a>(
    snapshot: &'a VaultSnapshot,
    path: &Path,
) -> Result<&'a str, PlanError> {
    snapshot.notes.get(path).map(String::as_str).ok_or_else(|| {
        PlanError::MissingNote {
            query: path.display().to_string(),
        }
    })
}

fn resolve_note(index: &NoteIndex, query: &str) -> Result<PathBuf, PlanError> {
    if query.trim().is_empty()
        || vault_links::target_to_markdown_path(query).is_none()
        || Path::new(query)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(PlanError::InvalidPath {
            query: query.to_string(),
        });
    }
    if query_is_explicit_path(query) {
        let Some(candidate) = vault_links::target_to_markdown_path(query)
        else {
            return Err(PlanError::InvalidPath {
                query: query.to_string(),
            });
        };
        return match exact_path_matches(index, &candidate).as_slice() {
            [path] => Ok(path.clone()),
            [] => Err(PlanError::MissingNote {
                query: query.to_string(),
            }),
            _ => Err(PlanError::AmbiguousNote {
                query: query.to_string(),
                candidates: exact_path_matches(index, &candidate),
            }),
        };
    }
    let candidates = index.basename_candidates(query);
    match candidates.as_slice() {
        [path] => Ok(path.clone()),
        [] => Err(PlanError::MissingNote {
            query: query.to_string(),
        }),
        _ => Err(PlanError::AmbiguousNote {
            query: query.to_string(),
            candidates,
        }),
    }
}

fn query_is_explicit_path(query: &str) -> bool {
    query.contains('/')
        || Path::new(query)
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

fn exact_path_matches(index: &NoteIndex, candidate: &Path) -> Vec<PathBuf> {
    let exact: Vec<PathBuf> = index
        .relative_paths()
        .filter(|path| *path == candidate)
        .map(Path::to_path_buf)
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    index
        .relative_paths()
        .filter(|path| paths_eq_ignore_ascii_case(path, candidate))
        .map(Path::to_path_buf)
        .collect()
}

fn paths_eq_ignore_ascii_case(left: &Path, right: &Path) -> bool {
    left.as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
}

fn refuse_archive_path(
    path: &Path,
    role: &'static str,
) -> Result<(), PlanError> {
    if is_archive_path(path) {
        return Err(PlanError::ArchivePath {
            path: path.to_path_buf(),
            role,
        });
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DestKind {
    Area,
    Project,
}

fn classify_dest(path: &Path, contents: &str) -> Result<DestKind, PlanError> {
    let Some(frontmatter) = parse_frontmatter(contents) else {
        return Err(PlanError::DestKind {
            path: path.to_path_buf(),
            detail: "destination is not an area or project note".to_string(),
        });
    };
    if frontmatter_is_project(&frontmatter) {
        match ProjectStatus::parse(frontmatter_value(&frontmatter, "status")) {
            ProjectStatus::Wip | ProjectStatus::Waiting => {
                Ok(DestKind::Project)
            }
            status => Err(PlanError::DestKind {
                path: path.to_path_buf(),
                detail: format!(
                    "project status is {}; destination must be wip or waiting",
                    status.label()
                ),
            }),
        }
    } else if frontmatter_is_area(&frontmatter) {
        Ok(DestKind::Area)
    } else {
        Err(PlanError::DestKind {
            path: path.to_path_buf(),
            detail: "destination is not an area or project note".to_string(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScannedTask {
    line_index: usize,
    indent: usize,
    symbol: char,
    block_id: Option<String>,
    title: String,
    line: String,
    in_fence_or_frontmatter: bool,
}

impl ScannedTask {
    fn candidate(&self) -> TaskCandidate {
        TaskCandidate {
            line: self.line_index + 1,
            title: self.title.clone(),
            block_id: self.block_id.clone(),
            status: self.symbol,
        }
    }

    fn is_column0(&self) -> bool {
        self.indent == 0
    }

    fn is_open_checkbox(&self) -> bool {
        OPEN_CHECKBOXES.contains(&self.symbol)
    }

    fn is_closed(&self) -> bool {
        matches!(self.symbol, 'x' | 'X' | '-')
    }

    fn is_prj(&self) -> bool {
        self.block_id.as_deref() == Some("prj")
    }
}

fn scan_task_lines(contents: &str) -> Vec<ScannedTask> {
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();
    let logical: Vec<&str> =
        lines.iter().map(|line| split_line_ending(line).0).collect();
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(&logical);
    let scan_start = frontmatter_end.map_or(0, |end| end + 1);
    let fenced = markdown::fenced_lines(&logical, scan_start..logical.len());
    let mut tasks = Vec::new();
    for (line_index, line) in logical.iter().enumerate() {
        let hidden = frontmatter_end.is_some_and(|end| line_index <= end)
            || fenced.contains(&line_index);
        let Some(parsed) = parse_task_line(line) else {
            continue;
        };
        tasks.push(ScannedTask {
            line_index,
            indent: parsed.indent,
            symbol: parsed.symbol,
            block_id: trailing_block_id_in_line(line),
            title: clean_description(
                parsed.body,
                GLOBAL_FILTER,
                trailing_block_id_in_line(line).as_deref(),
            ),
            line: line.to_string(),
            in_fence_or_frontmatter: hidden,
        });
    }
    tasks
}

struct ParsedTaskLine<'a> {
    indent: usize,
    symbol: char,
    body: &'a str,
}

fn parse_task_line(line: &str) -> Option<ParsedTaskLine<'_>> {
    let indent = leading_indent_len(line);
    let rest = line.get(indent..)?;
    let rest = rest.strip_prefix("- [")?;
    let symbol = rest.chars().next()?;
    let after = rest.get(symbol.len_utf8()..)?.strip_prefix(']')?;
    if !after.is_empty() && !after.starts_with(char::is_whitespace) {
        return None;
    }
    if !has_task_tag(line) {
        return None;
    }
    Some(ParsedTaskLine {
        indent,
        symbol,
        body: after.trim_start(),
    })
}

fn select_tasks(
    contents: &str,
    tasks: &[ScannedTask],
    selectors: &[MoveSelector],
) -> Result<Vec<ScannedTask>, PlanError> {
    let visible: Vec<&ScannedTask> = tasks
        .iter()
        .filter(|task| !task.in_fence_or_frontmatter)
        .collect();
    let mut selected: BTreeMap<usize, ScannedTask> = BTreeMap::new();
    for selector in selectors {
        let matched = match selector {
            MoveSelector::Block(id) => visible
                .iter()
                .copied()
                .filter(|task| task.block_id.as_deref() == Some(id.as_str()))
                .collect::<Vec<_>>(),
            MoveSelector::Match(text) => visible
                .iter()
                .copied()
                .filter(|task| task.line.contains(text))
                .collect::<Vec<_>>(),
        };

        if let MoveSelector::Block(id) = selector
            && matched.is_empty()
        {
            let hits = lines_with_block_id(contents, id);
            if let Some((line, excerpt)) = hits.first() {
                return Err(PlanError::NotATask {
                    line: line + 1,
                    excerpt: excerpt.clone(),
                    block_id: id.clone(),
                });
            }
        }

        let unique = resolve_selector_matches(selector, &matched, tasks)?;
        selected.insert(unique.line_index, unique.clone());
    }
    Ok(selected.into_values().collect())
}

fn lines_with_block_id(contents: &str, id: &str) -> Vec<(usize, String)> {
    contents
        .split_inclusive('\n')
        .enumerate()
        .filter_map(|(index, line)| {
            let (content, _) = split_line_ending(line);
            (trailing_block_id_in_line(content).as_deref() == Some(id))
                .then(|| (index, content.trim().to_string()))
        })
        .collect()
}

fn resolve_selector_matches(
    selector: &MoveSelector,
    matched: &[&ScannedTask],
    all_tasks: &[ScannedTask],
) -> Result<ScannedTask, PlanError> {
    let candidates: Vec<TaskCandidate> =
        matched.iter().map(|task| task.candidate()).collect();
    if matched.is_empty() {
        return Err(PlanError::SelectorZero {
            selector: selector.clone(),
            candidates: all_tasks
                .iter()
                .filter(|task| !task.in_fence_or_frontmatter)
                .map(ScannedTask::candidate)
                .collect(),
        });
    }
    if matched.len() > 1 {
        return Err(PlanError::SelectorAmbiguous {
            selector: selector.clone(),
            candidates,
        });
    }
    Ok(matched[0].clone())
}

fn validate_movable_task(task: &ScannedTask) -> Result<(), PlanError> {
    if !task.is_column0() {
        return Err(PlanError::NestedTask {
            candidate: task.candidate(),
        });
    }
    if task.is_prj() {
        return Err(PlanError::ProjectTask {
            candidate: task.candidate(),
        });
    }
    if task.is_closed() {
        return Err(PlanError::ClosedTask {
            candidate: task.candidate(),
        });
    }
    if !task.is_open_checkbox() {
        return Err(PlanError::UnsupportedCheckbox {
            candidate: task.candidate(),
        });
    }
    Ok(())
}

fn task_depends_on_dest_prj(
    lines: &[&str],
    fenced: &BTreeSet<usize>,
    task_line: usize,
    source_path: &Path,
    dest_path: &Path,
    note_index: &NoteIndex,
) -> bool {
    let Some(child) = parse::dependency_child_of(lines, fenced, task_line)
    else {
        return false;
    };
    let DependencyLine::Accepted { links, .. } = child.parsed else {
        return false;
    };
    links.iter().any(|link| {
        if link.struck || link.block_id != "prj" {
            return false;
        }
        note_index
            .resolve(Some(source_path), &link.target)
            .as_deref()
            == Some(dest_path)
    })
}

fn dest_prj_is_blocked(contents: &str) -> bool {
    let tasks = scan_task_lines(contents);
    tasks.iter().any(|task| {
        task.is_column0()
            && task.is_prj()
            && !task.in_fence_or_frontmatter
            && (task.symbol == '?' || has_prj_is_blocked_tag(&task.line))
    })
}

fn count_live_block_ids(contents: &str) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for id in live_block_ids(contents) {
        *counts.entry(id).or_insert(0) += 1;
    }
    counts
}

fn live_block_ids(contents: &str) -> Vec<String> {
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();
    let logical: Vec<&str> =
        lines.iter().map(|line| split_line_ending(line).0).collect();
    let hidden = hidden_markdown_indexes(&logical);
    logical
        .iter()
        .enumerate()
        .filter(|(index, _)| !hidden.contains(index))
        .filter_map(|(_, line)| trailing_block_id_in_line(line))
        .collect()
}

fn hidden_markdown_indexes(logical: &[&str]) -> BTreeSet<usize> {
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(logical);
    let scan_start = frontmatter_end.map_or(0, |end| end + 1);
    let mut hidden = markdown::fenced_lines(logical, scan_start..logical.len());
    if let Some(end) = frontmatter_end {
        hidden.extend(0..=end);
    }
    hidden
}

fn live_id_fields(line: &str) -> Vec<crate::native::task_fields::InlineField> {
    let spans = inline_code_spans(line);
    inline_fields(line, "id")
        .into_iter()
        .filter(|field| {
            !spans
                .iter()
                .any(|span| field.start < span.end && field.end > span.start)
        })
        .collect()
}

fn required_dependency_id(
    path: &Path,
    block_id: &str,
) -> Result<String, PlanError> {
    dependency_id(path, block_id).map_err(|error| {
        PlanError::UnrepresentableIdentity {
            path: path.to_path_buf(),
            block_id: block_id.to_string(),
            detail: error.to_string(),
        }
    })
}

fn validate_mapped_identities(
    snapshot: &VaultSnapshot,
    source_path: &Path,
    dest_path: &Path,
    id_map: &BTreeMap<String, String>,
) -> Result<(), PlanError> {
    let mut mapped = Vec::new();
    for (old_id, new_id) in id_map {
        mapped.push((
            old_id.clone(),
            new_id.clone(),
            required_dependency_id(source_path, old_id)?,
            required_dependency_id(dest_path, new_id)?,
        ));
    }

    let mut owners: BTreeMap<String, Vec<(PathBuf, String)>> = BTreeMap::new();
    for (path, contents) in &snapshot.notes {
        for block_id in live_block_ids(contents) {
            if let Ok(token) = dependency_id(path, &block_id) {
                owners
                    .entry(token)
                    .or_default()
                    .push((path.clone(), block_id));
            }
        }
        let logical: Vec<&str> = contents
            .split_inclusive('\n')
            .map(|line| split_line_ending(line).0)
            .collect();
        let hidden = hidden_markdown_indexes(&logical);
        for (index, line) in logical.iter().enumerate() {
            if hidden.contains(&index) || parse_task_line(line).is_none() {
                continue;
            }
            let block_id = trailing_block_id_in_line(line).unwrap_or_default();
            for field in live_id_fields(line) {
                owners
                    .entry(field.value)
                    .or_default()
                    .push((path.clone(), block_id.clone()));
            }
        }
    }

    for (old_id, _new_id, old_dep, new_dep) in mapped {
        for token in [old_dep, new_dep] {
            let Some(list) = owners.get(&token) else {
                continue;
            };
            let foreign: Vec<&(PathBuf, String)> = list
                .iter()
                .filter(|(path, block_id)| {
                    !(path == source_path && block_id == &old_id)
                })
                .collect();
            if let Some((path, block_id)) = foreign.first() {
                return Err(PlanError::EncodedIdentityCollision {
                    token,
                    first: source_path.to_path_buf(),
                    first_block: old_id,
                    second: (*path).clone(),
                    second_block: (*block_id).clone(),
                });
            }
        }
    }
    Ok(())
}

fn validate_fragment_identities(
    fragment: &str,
    source_path: &Path,
    source_counts: &BTreeMap<String, usize>,
    owner: &ScannedTask,
) -> Result<(), PlanError> {
    let lines: Vec<&str> = fragment.split_inclusive('\n').collect();
    let logical: Vec<&str> =
        lines.iter().map(|line| split_line_ending(line).0).collect();
    let hidden = hidden_markdown_indexes(&logical);
    for (offset, content) in logical.iter().enumerate() {
        if hidden.contains(&offset) {
            continue;
        }
        let Some(block_id) = trailing_block_id_in_line(content) else {
            if parse_task_line(content).is_some()
                && let Some(field) = live_id_fields(content).into_iter().next()
            {
                return Err(PlanError::NonCanonicalId {
                    candidate: candidate_from_line(
                        owner.line_index + offset,
                        content,
                    ),
                    found: field.value,
                    expected: "an id derived from a block anchor".to_string(),
                });
            }
            continue;
        };
        if source_counts.get(&block_id).copied().unwrap_or(0) != 1 {
            return Err(PlanError::AmbiguousIdentity {
                path: source_path.to_path_buf(),
                block_id,
            });
        }
        if parse_task_line(content).is_none() {
            continue;
        }
        let expected = required_dependency_id(source_path, &block_id)?;
        for field in live_id_fields(content) {
            if field.value != expected {
                return Err(PlanError::NonCanonicalId {
                    candidate: candidate_from_line(
                        owner.line_index + offset,
                        content,
                    ),
                    found: field.value,
                    expected,
                });
            }
        }
    }
    Ok(())
}

fn candidate_from_line(line_index: usize, line: &str) -> TaskCandidate {
    let parsed = parse_task_line(line);
    TaskCandidate {
        line: line_index + 1,
        title: parsed
            .as_ref()
            .map(|parsed| {
                clean_description(
                    parsed.body,
                    GLOBAL_FILTER,
                    trailing_block_id_in_line(line).as_deref(),
                )
            })
            .unwrap_or_else(|| line.trim().to_string()),
        block_id: trailing_block_id_in_line(line),
        status: parsed.map(|parsed| parsed.symbol).unwrap_or(' '),
    }
}

struct SelectedBlock {
    task: ScannedTask,
    end: usize,
    fragment: String,
}

fn rename_incoming_fragments(
    blocks: &[SelectedBlock],
    dest_path: &Path,
    dest_used: &BTreeSet<String>,
) -> Result<(Vec<String>, Vec<RenameRecord>, BTreeMap<String, String>), PlanError>
{
    let mut used = dest_used.clone();
    let mut reserved = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for block in blocks {
        for occurrence in fragment_occurrences(&block.fragment) {
            if seen.insert(occurrence.id.clone())
                && !used.contains(&occurrence.id)
            {
                reserved.insert(occurrence.id);
            }
        }
    }

    let mut fragments = Vec::new();
    let mut renames = Vec::new();
    let mut id_map = BTreeMap::new();
    for block in blocks {
        let occurrences = fragment_occurrences(&block.fragment);
        let mut finals = Vec::with_capacity(occurrences.len());
        for occurrence in &occurrences {
            let final_id = if used.contains(&occurrence.id) {
                next_available_block_id(&occurrence.id, &used, &reserved)
            } else {
                occurrence.id.clone()
            };
            if final_id != occurrence.id {
                renames.push(RenameRecord {
                    path: dest_path.to_path_buf(),
                    old_block_id: occurrence.id.clone(),
                    new_block_id: final_id.clone(),
                });
            }
            used.insert(final_id.clone());
            id_map.insert(occurrence.id.clone(), final_id.clone());
            finals.push(final_id);
        }
        let mut fragment = block.fragment.clone();
        for (occurrence, final_id) in
            occurrences.iter().zip(finals.iter()).rev()
        {
            if final_id != &occurrence.id {
                fragment
                    .replace_range(occurrence.start..occurrence.end, final_id);
            }
        }
        fragments.push(fragment);
    }
    Ok((fragments, renames, id_map))
}

struct Occurrence {
    id: String,
    start: usize,
    end: usize,
}

fn fragment_occurrences(fragment: &str) -> Vec<Occurrence> {
    let lines: Vec<&str> = fragment.split_inclusive('\n').collect();
    let logical: Vec<&str> =
        lines.iter().map(|line| split_line_ending(line).0).collect();
    let hidden = hidden_markdown_indexes(&logical);
    let mut occurrences = Vec::new();
    let mut offset = 0;
    for (index, line) in lines.iter().enumerate() {
        let (content, _) = split_line_ending(line);
        if !hidden.contains(&index) {
            for occurrence in block_id_occurrences_in_text(content) {
                occurrences.push(Occurrence {
                    id: occurrence.id,
                    start: offset + occurrence.start,
                    end: offset + occurrence.end,
                });
            }
        }
        offset += line.len();
    }
    occurrences
}

struct FragmentRepair {
    contents: String,
    link_count: usize,
    dependency_count: usize,
}

fn repair_moved_fragment(
    fragment: &str,
    source_path: &Path,
    source_target: &str,
    note_index: &NoteIndex,
    moved_targets: &MovedBlockTargets,
    stayed_source_ids: &BTreeSet<String>,
    id_map: &BTreeMap<String, String>,
) -> FragmentRepair {
    let wiki = repair_fragment_wiki_links(
        fragment,
        source_path,
        source_target,
        note_index,
        moved_targets,
        stayed_source_ids,
        id_map,
    );
    let markdown = repair_fragment_markdown_links(
        &wiki.contents,
        source_path,
        source_target,
        note_index,
        moved_targets,
        stayed_source_ids,
        id_map,
    );
    let metadata =
        repair_dependency_metadata(&markdown.contents, moved_targets);
    FragmentRepair {
        contents: metadata.contents,
        link_count: wiki.link_count + markdown.link_count,
        dependency_count: metadata.count,
    }
}

fn repair_fragment_wiki_links(
    contents: &str,
    source_path: &Path,
    source_target: &str,
    note_index: &NoteIndex,
    moved_targets: &MovedBlockTargets,
    stayed_source_ids: &BTreeSet<String>,
    id_map: &BTreeMap<String, String>,
) -> collect_done::LinkRepairResult {
    let lines: Vec<&str> = contents
        .split_inclusive('\n')
        .map(|line| split_line_ending(line).0)
        .collect();
    let fenced = markdown::fenced_lines(&lines, 0..lines.len());
    let mut repaired = String::with_capacity(contents.len());
    let mut link_count = 0;
    let mut cursor = 0;
    while let Some(relative_start) = contents[cursor..].find("[[") {
        let start = cursor + relative_start;
        let inner_start = start + 2;
        let Some(relative_end) = contents[inner_start..].find("]]") else {
            break;
        };
        let end = inner_start + relative_end;
        repaired.push_str(&contents[cursor..start]);
        let inner = &contents[inner_start..end];
        let line_index = contents[..start].matches('\n').count();
        let line = lines.get(line_index).copied().unwrap_or("");
        let line_start =
            contents[..start].rfind('\n').map_or(0, |index| index + 1);
        let code_spans = inline_code_spans(line);
        let relative_in_line = start.saturating_sub(line_start);
        let skip = fenced.contains(&line_index)
            || code_spans.iter().any(|span| {
                relative_in_line < span.end && end + 2 - line_start > span.start
            });
        let replacement = if skip {
            None
        } else {
            repair_fragment_wiki_inner(
                inner,
                source_path,
                source_target,
                note_index,
                moved_targets,
                stayed_source_ids,
                id_map,
            )
        };
        if let Some(new_inner) = replacement {
            repaired.push_str("[[");
            repaired.push_str(&new_inner);
            repaired.push_str("]]");
            link_count += 1;
        } else {
            repaired.push_str(&contents[start..end + 2]);
        }
        cursor = end + 2;
    }
    repaired.push_str(&contents[cursor..]);
    collect_done::LinkRepairResult {
        contents: repaired,
        link_count,
        dependency_metadata_count: 0,
    }
}

fn repair_fragment_wiki_inner(
    inner: &str,
    source_path: &Path,
    source_target: &str,
    note_index: &NoteIndex,
    moved_targets: &MovedBlockTargets,
    stayed_source_ids: &BTreeSet<String>,
    id_map: &BTreeMap<String, String>,
) -> Option<String> {
    let (target_with_fragment, alias) = match inner.find('|') {
        Some(index) => (&inner[..index], &inner[index..]),
        None => (inner, ""),
    };
    let (target, fragment) = split_block_fragment(target_with_fragment)?;
    let block_id = fragment.strip_prefix('^')?;
    if target.trim().is_empty() {
        if let Some(final_id) = id_map.get(block_id) {
            return Some(format!("#^{final_id}{alias}"));
        }
        if stayed_source_ids.contains(block_id) {
            return Some(format!("{source_target}#^{block_id}{alias}"));
        }
        return None;
    }
    repair_wiki_link_inner(inner, source_path, note_index, moved_targets)
}

fn repair_fragment_markdown_links(
    contents: &str,
    source_path: &Path,
    source_target: &str,
    note_index: &NoteIndex,
    moved_targets: &MovedBlockTargets,
    stayed_source_ids: &BTreeSet<String>,
    id_map: &BTreeMap<String, String>,
) -> collect_done::LinkRepairResult {
    let lines: Vec<&str> = contents
        .split_inclusive('\n')
        .map(|line| split_line_ending(line).0)
        .collect();
    let fenced = markdown::fenced_lines(&lines, 0..lines.len());
    let mut repaired = String::with_capacity(contents.len());
    let mut link_count = 0;
    let mut cursor = 0;
    while let Some(relative_open) = contents[cursor..].find("](") {
        let open = cursor + relative_open;
        if let Some(wiki_start) =
            contents[cursor..].find("[[").map(|rel| cursor + rel)
            && wiki_start < open
        {
            let Some(wiki_end) = contents[wiki_start + 2..].find("]]") else {
                break;
            };
            let wiki_end = wiki_start + 2 + wiki_end + 2;
            repaired.push_str(&contents[cursor..wiki_end]);
            cursor = wiki_end;
            continue;
        }
        let destination_start = open + 2;
        let Some(relative_close) = contents[destination_start..].find(')')
        else {
            break;
        };
        let destination_end = destination_start + relative_close;
        repaired.push_str(&contents[cursor..destination_start]);
        let destination = &contents[destination_start..destination_end];
        let line_index = contents[..open].matches('\n').count();
        let line = lines.get(line_index).copied().unwrap_or("");
        let line_start =
            contents[..open].rfind('\n').map_or(0, |index| index + 1);
        let code_spans = inline_code_spans(line);
        let relative_in_line = open.saturating_sub(line_start);
        let skip = fenced.contains(&line_index)
            || code_spans.iter().any(|span| {
                relative_in_line < span.end
                    && destination_end + 1 - line_start > span.start
            });
        if skip {
            repaired.push_str(destination);
        } else if let Some(new_destination) =
            repair_fragment_markdown_destination(
                destination,
                source_path,
                source_target,
                note_index,
                moved_targets,
                stayed_source_ids,
                id_map,
            )
        {
            repaired.push_str(&new_destination);
            link_count += 1;
        } else {
            repaired.push_str(destination);
        }
        repaired.push(')');
        cursor = destination_end + 1;
    }
    repaired.push_str(&contents[cursor..]);
    collect_done::LinkRepairResult {
        contents: repaired,
        link_count,
        dependency_metadata_count: 0,
    }
}

fn repair_fragment_markdown_destination(
    destination: &str,
    source_path: &Path,
    source_target: &str,
    note_index: &NoteIndex,
    moved_targets: &MovedBlockTargets,
    stayed_source_ids: &BTreeSet<String>,
    id_map: &BTreeMap<String, String>,
) -> Option<String> {
    if !is_simple_markdown_destination(destination) {
        return None;
    }
    let (target, fragment) = split_block_fragment(destination)?;
    if target.contains("://") {
        return None;
    }
    let block_id = fragment.strip_prefix('^')?;
    if target.is_empty() {
        if let Some(final_id) = id_map.get(block_id) {
            return Some(format!("#^{final_id}"));
        }
        if stayed_source_ids.contains(block_id) {
            return Some(format!("{source_target}.md#^{block_id}"));
        }
        return None;
    }
    repair_markdown_destination(
        destination,
        source_path,
        note_index,
        moved_targets,
    )
}

fn remove_selected_blocks(contents: &str, blocks: &[SelectedBlock]) -> String {
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();
    let mut drop = vec![false; lines.len()];
    for block in blocks {
        for index in block.task.line_index..block.end {
            if index < drop.len() {
                drop[index] = true;
            }
        }
    }
    let mut result = String::new();
    let mut last_emitted_blank = false;
    let mut at_seam = false;
    for (index, line) in lines.iter().enumerate() {
        if drop[index] {
            at_seam = true;
            continue;
        }
        let blank = split_line_ending(line).0.trim().is_empty();
        if at_seam && blank && last_emitted_blank {
            at_seam = false;
            continue;
        }
        at_seam = false;
        result.push_str(line);
        last_emitted_blank = blank;
    }
    result
}

fn insert_into_dest(
    dest_contents: &str,
    fragments: &[String],
    dest_kind: DestKind,
) -> Result<String, PlanError> {
    let insertion =
        join_fragments(fragments, document_line_ending(dest_contents));
    if let Some(insert_at) = tasks_intake_insert_byte(dest_contents) {
        return Ok(insert_at_byte(dest_contents, insert_at, &insertion));
    }
    if dest_kind == DestKind::Area {
        return Ok(append_tasks_section(dest_contents, &insertion));
    }
    Err(PlanError::ProjectWithoutTasks {
        path: PathBuf::from("destination"),
    })
}

fn join_fragments(fragments: &[String], ending: &str) -> String {
    let mut joined = String::new();
    for fragment in fragments {
        if !joined.is_empty() && !joined.ends_with('\n') {
            joined.push_str(ending);
        }
        joined.push_str(fragment);
    }
    if !joined.is_empty() && !joined.ends_with('\n') {
        joined.push_str(ending);
    }
    joined
}

fn append_tasks_section(contents: &str, insertion: &str) -> String {
    let ending = document_line_ending(contents);
    let mut result = contents.to_string();
    if !result.is_empty() && !result.ends_with('\n') {
        result.push_str(ending);
    }
    if !result.is_empty() {
        let already_blank = result.ends_with(&format!("{ending}{ending}"));
        if !already_blank {
            result.push_str(ending);
        }
    }
    result.push_str("## Tasks");
    result.push_str(ending);
    result.push_str(ending);
    result.push_str(insertion);
    result
}

fn tasks_heading_index(contents: &str) -> Option<usize> {
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();
    let logical: Vec<&str> =
        lines.iter().map(|line| split_line_ending(line).0).collect();
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(&logical);
    let scan_start = frontmatter_end.map_or(0, |end| end + 1);
    let fenced = markdown::fenced_lines(&logical, scan_start..logical.len());
    logical.iter().enumerate().find_map(|(index, line)| {
        if frontmatter_end.is_some_and(|end| index <= end)
            || fenced.contains(&index)
        {
            return None;
        }
        matches!(markdown::atx_heading(line), Some((2, title)) if title == "Tasks")
            .then_some(index)
    })
}

fn tasks_intake_insert_byte(contents: &str) -> Option<usize> {
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();
    let logical: Vec<&str> =
        lines.iter().map(|line| split_line_ending(line).0).collect();
    let heading = tasks_heading_index(contents)?;
    let mut body_start = heading + 1;
    if body_start < logical.len()
        && task_status_groups::is_legacy_badge_marker(logical[body_start])
    {
        body_start += 1;
    }
    if body_start < logical.len()
        && task_status_groups::is_badge_row(logical[body_start])
    {
        body_start += 1;
    }
    let fenced = markdown::fenced_lines(&logical, body_start..logical.len());
    let mut intake_end = logical.len();
    for (index, line) in logical.iter().enumerate().skip(body_start) {
        if fenced.contains(&index) {
            continue;
        }
        if let Some((level, _)) = markdown::atx_heading(line)
            && level <= 3
        {
            intake_end = index;
            break;
        }
    }
    Some(lines.iter().take(intake_end).map(|line| line.len()).sum())
}

fn insert_at_byte(contents: &str, index: usize, insertion: &str) -> String {
    let ending = document_line_ending(contents);
    let mut addition = insertion.to_string();
    let needs_leading = index > 0 && !contents[..index].ends_with('\n');
    if needs_leading {
        addition = format!("{ending}{addition}");
    }
    if index == 0 || contents[..index].ends_with(&format!("{ending}{ending}")) {
        // already a blank line before the insert point
    } else if last_line_is_heading(&contents[..index]) {
        addition = format!("{ending}{addition}");
    }
    let mut result = String::with_capacity(contents.len() + addition.len());
    result.push_str(&contents[..index]);
    result.push_str(&addition);
    result.push_str(&contents[index..]);
    result
}

fn last_line_is_heading(prefix: &str) -> bool {
    let last = prefix
        .rsplit_once('\n')
        .map(|(_, last)| split_line_ending(last).0)
        .unwrap_or_else(|| split_line_ending(prefix).0);
    let last = if last.is_empty() {
        prefix
            .rsplit('\n')
            .nth(1)
            .map(|line| split_line_ending(line).0)
            .unwrap_or("")
    } else {
        last
    };
    markdown::atx_heading(last).is_some()
        || task_status_groups::is_badge_row(last)
        || task_status_groups::is_legacy_badge_marker(last)
}

fn document_line_ending(contents: &str) -> &'static str {
    if contents.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn add_repair(
    counts: &mut BTreeMap<PathBuf, (usize, usize)>,
    path: PathBuf,
    links: usize,
    deps: usize,
) {
    let entry = counts.entry(path).or_insert((0, 0));
    entry.0 += links;
    entry.1 += deps;
}

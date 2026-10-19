use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::{Component, Path, PathBuf},
};

#[cfg(test)]
use std::{fs, io};

use super::super::{
    capture_dependency_tasks::has_prj_is_blocked_tag,
    collect_done::{
        self, block_id_occurrences_in_text, block_ids_in_markdown,
        has_task_tag, is_simple_markdown_destination, leading_indent_len,
        next_available_block_id, repair_dependency_metadata,
        repair_links_in_note, repair_markdown_destination,
        repair_wiki_link_inner, split_block_fragment, split_line_ending,
        task_block_end, trailing_block_id_in_line, vault_relative_link_target,
        ArchiveNoteIndex, MovedBlockTarget, MovedBlockTargets,
    },
    markdown,
    note_tasks::clean_description,
    projects::{
        frontmatter_is_area, frontmatter_is_project, frontmatter_value,
        parse_frontmatter, ProjectStatus,
    },
    task_dependencies::{
        dependency_id, inline_code_spans, is_archive_path, parse,
        DependencyLine,
    },
    task_fields::inline_fields,
    task_status_groups,
    vault_links::{self, LinkResolution, NoteIndex},
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
    let archive_index =
        ArchiveNoteIndex::from_paths(snapshot.notes.keys().cloned());
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
            &archive_index,
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

    let source_id_counts = count_block_ids(source_contents);
    let mut incoming_ids = Vec::new();
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
        for id in block_ids_in_markdown(&fragment) {
            incoming_ids.push(id);
        }
        blocks.push(SelectedBlock {
            task: task.clone(),
            end,
            fragment,
        });
    }

    let dest_used: BTreeSet<String> =
        block_ids_in_markdown(dest_contents).into_iter().collect();
    let (fragments, renames, id_map) =
        rename_incoming_fragments(&blocks, &dest_path, &dest_used)?;

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
        let old_dep = dependency_id(&source_path, old_id).unwrap_or_default();
        let new_dep = dependency_id(&dest_path, new_id).unwrap_or_default();
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
    let stayed_source_ids: BTreeSet<String> = block_ids_in_markdown(
        &remove_selected_blocks(source_contents, &blocks),
    )
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
            &archive_index,
            &moved_targets,
            &stayed_source_ids,
            &id_map,
        );
        fragment_link_count += repaired.link_count;
        fragment_dep_count += repaired.dependency_count;
        repaired_fragments.push(repaired.contents);
    }

    let dest_existing_repair = repair_links_in_note(
        dest_contents,
        &dest_path,
        &archive_index,
        &moved_targets,
    );
    let source_without = remove_selected_blocks(source_contents, &blocks);
    let source_repair = repair_links_in_note(
        &source_without,
        &source_path,
        &archive_index,
        &moved_targets,
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
        let repair = repair_links_in_note(
            original,
            path,
            &archive_index,
            &moved_targets,
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
    match index.resolve_detailed(None, query) {
        LinkResolution::Found(path) => Ok(path),
        LinkResolution::Ambiguous => Err(PlanError::AmbiguousNote {
            query: query.to_string(),
            candidates: index.basename_candidates(query),
        }),
        LinkResolution::Missing => Err(PlanError::MissingNote {
            query: query.to_string(),
        }),
    }
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
    note_index: &ArchiveNoteIndex,
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
        note_index.resolve(source_path, &link.target).as_deref()
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

fn count_block_ids(contents: &str) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for id in block_ids_in_markdown(contents) {
        *counts.entry(id).or_insert(0) += 1;
    }
    counts
}

fn validate_fragment_identities(
    fragment: &str,
    source_path: &Path,
    source_counts: &BTreeMap<String, usize>,
    owner: &ScannedTask,
) -> Result<(), PlanError> {
    let lines: Vec<&str> = fragment.split_inclusive('\n').collect();
    for (offset, line) in lines.iter().enumerate() {
        let (content, _) = split_line_ending(line);
        let Some(block_id) = trailing_block_id_in_line(content) else {
            if let Some(field) = inline_fields(content, "id").into_iter().next()
                && parse_task_line(content).is_some()
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
        let Some(task) = parse_task_line(content) else {
            continue;
        };
        let expected = dependency_id(source_path, &block_id).map_err(|_| {
            PlanError::NonCanonicalId {
                candidate: candidate_from_line(
                    owner.line_index + offset,
                    content,
                ),
                found: inline_fields(content, "id")
                    .into_iter()
                    .next()
                    .map(|field| field.value)
                    .unwrap_or_default(),
                expected: format!(
                    "dependency_id({}, {block_id})",
                    source_path.display()
                ),
            }
        })?;
        if let Some(field) = inline_fields(content, "id").into_iter().next()
            && field.value != expected
        {
            return Err(PlanError::NonCanonicalId {
                candidate: candidate_from_line(
                    owner.line_index + offset,
                    content,
                ),
                found: field.value,
                expected,
            });
        }
        let _ = task;
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
    let mut occurrences = Vec::new();
    let mut offset = 0;
    for line in fragment.split_inclusive('\n') {
        let (content, _) = split_line_ending(line);
        for occurrence in block_id_occurrences_in_text(content) {
            occurrences.push(Occurrence {
                id: occurrence.id,
                start: offset + occurrence.start,
                end: offset + occurrence.end,
            });
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
    note_index: &ArchiveNoteIndex,
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
    note_index: &ArchiveNoteIndex,
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
    note_index: &ArchiveNoteIndex,
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
    note_index: &ArchiveNoteIndex,
    moved_targets: &MovedBlockTargets,
    stayed_source_ids: &BTreeSet<String>,
    id_map: &BTreeMap<String, String>,
) -> collect_done::LinkRepairResult {
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
        if let Some(new_destination) = repair_fragment_markdown_destination(
            destination,
            source_path,
            source_target,
            note_index,
            moved_targets,
            stayed_source_ids,
            id_map,
        ) {
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
    note_index: &ArchiveNoteIndex,
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
    let mut intake_end = logical.len();
    for (index, line) in logical.iter().enumerate().skip(body_start) {
        if let Some((level, _)) = markdown::atx_heading(line)
            && level <= 3
        {
            intake_end = index;
            break;
        }
    }
    let mut last_task_end = None;
    for index in body_start..intake_end {
        if parse_task_line(logical[index]).is_some_and(|task| task.indent == 0)
        {
            last_task_end = Some(task_block_end(&lines, index, 0));
        }
    }
    let insert_line = last_task_end.unwrap_or(body_start);
    Some(lines.iter().take(insert_line).map(|line| line.len()).sum())
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

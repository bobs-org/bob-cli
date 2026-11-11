use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::{Component, Path, PathBuf},
};

use serde::Serialize;

use super::transplant;
pub(crate) use super::transplant::{RenameRecord, RepairedFile};

#[cfg(test)]
use std::{fs, io};

use super::super::{
    capture_dependency_tasks::has_prj_is_blocked_tag,
    capture_task_toggle::set_task_line_status,
    collect_done::pair::{ArchiveLookup, ArchivePairIndex},
    collect_done::{
        has_task_tag, leading_indent_len, repair_links_in_note_with,
        split_line_ending, task_block_end, trailing_block_id_in_line,
        vault_relative_link_target, MovedBlockTarget, MovedBlockTargets,
    },
    markdown,
    note_tasks::clean_description,
    pomodoro_homes::{HomeResolver, IndexedHomeSource},
    projects::{
        frontmatter_is_album, frontmatter_is_area, frontmatter_is_notebook,
        frontmatter_is_project, frontmatter_value, parse_frontmatter,
        trim_yaml_scalar, ProjectStatus,
    },
    relation_labels::relation_label_key,
    task_complete::tree::remove_completion_fields,
    task_complete::{is_inbox_note, is_recurring_task_line},
    task_dependencies::{
        dependency_id, inline_code_spans, is_archive_path, parse,
        DependencyLine,
    },
    task_fields::inline_fields,
    task_relations, task_status_groups,
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
    /// Accept closed selections. Only capture inbox routing sets this:
    /// completions route home before archiving, so the engine must move
    /// `[x]`/`[-]` tasks it otherwise refuses. The `bob task move` CLI
    /// never sets it and keeps refusing closed tasks.
    pub(crate) allow_closed: bool,
    /// Capture can place moved roots into the order represented by its
    /// project-note draft. Each slot is the root-task index in the final
    /// `## Tasks` lane and is paired with the selector at the same index.
    /// The regular task-move command and inbox routing keep appending.
    pub(crate) placement: MovePlacement,
    /// The regular task-move command and inbox routing preserve the existing
    /// collision-renaming behavior. Capture can refuse instead because a
    /// collision would make a typed move bullet resolve to a different task.
    pub(crate) id_collision: IdCollisionPolicy,
    /// Restore an archived task: the source may live under done/ (the
    /// destination still may not), an empty destination resolves to the
    /// archive's bound source through the pairing resolver, and recurring
    /// occurrences plus merge tombstones are refused. Only
    /// `bob task restore` sets this; `bob task move`, inbox routing, and
    /// capture keep refusing every done/ path.
    pub(crate) restore: bool,
    /// Reopen each restored root (`[ ]` plus close-field removal). Only
    /// `bob task restore --reopen` sets this.
    pub(crate) reopen: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MovePlacement {
    Append,
    CaptureSlots(Vec<usize>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdCollisionPolicy {
    Rename,
    Refuse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MoveSelector {
    Block(String),
    Match(String),
    /// Exact 0-based line index in the source snapshot. Only capture
    /// inbox routing uses this, for candidates without a block ID where
    /// fuzzy text matching would be wrong. The CLI never constructs it.
    Line(usize),
}

impl fmt::Display for MoveSelector {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Block(id) => write!(formatter, "block id {id}"),
            Self::Match(text) => write!(formatter, "match {text:?}"),
            Self::Line(index) => write!(formatter, "line {}", index + 1),
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
    crate::native::done_archive::is_archive_path(path)
        || path.components().any(|component| {
            matches!(
                component,
                Component::Normal(name)
                    if name == "_templates" || name == "_conflicts"
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
    /// Exact 1-based landing line in the destination postimage. Carries
    /// identity for id-less moves when the destination already holds
    /// the same title/status or several identical roots move together,
    /// so callers never search by unique title/status text.
    pub(crate) dest_line: usize,
    pub(crate) title: String,
    pub(crate) old_block_id: Option<String>,
    pub(crate) new_block_id: Option<String>,
    pub(crate) old_dependency_id: Option<String>,
    pub(crate) new_dependency_id: Option<String>,
    pub(crate) destination_section: Option<String>,
    pub(crate) status_symbol: char,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskCandidate {
    pub(crate) line: usize,
    pub(crate) title: String,
    pub(crate) block_id: Option<String>,
    pub(crate) status: char,
}

/// A top-level, non-closed task that capture may offer for moving, along with
/// any source-side reason the move engine would refuse it. Destination rules
/// are checked when the move is planned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MovableRoot {
    pub(crate) line: usize,
    pub(crate) title: String,
    pub(crate) block_id: Option<String>,
    pub(crate) status: char,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) disabled_reason: Option<String>,
}

/// List movable-root candidates using the same source-side eligibility checks
/// as `plan_task_move`. ID-less tasks remain eligible and are returned with no
/// block ID so capture can offer its Add block ID flow.
pub(crate) fn movable_roots(
    source_path: &Path,
    contents: &str,
) -> Vec<MovableRoot> {
    let tasks = scan_task_lines(contents);
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();
    let id_counts = count_live_block_ids(contents);
    tasks
        .iter()
        .filter(|task| {
            !task.in_fence_or_frontmatter
                && task.is_column0()
                && !task.is_closed()
        })
        .map(|task| {
            let reason = validate_movable_root(
                task,
                source_path,
                &lines,
                &id_counts,
                false,
            )
            .err()
            .map(|error| error.to_string());
            MovableRoot {
                line: task.line_index + 1,
                title: task.title.clone(),
                block_id: task.block_id.clone(),
                status: task.symbol,
                disabled_reason: reason,
            }
        })
        .collect()
}

/// A typed reason a capture move ID does not resolve to one movable root.
/// These cases are derived from the same task scanner and eligibility rules
/// used by the move engine, so capture can give stable teaching errors
/// without parsing an engine error's display text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Ineligible {
    Missing,
    Closed {
        status: char,
    },
    Nested {
        parent_id: Option<String>,
        parent_title: String,
    },
    Ambiguous {
        count: usize,
    },
    Project,
    UnsupportedCheckbox {
        status: char,
    },
}

/// Resolve one typed block ID against the source note. Duplicate task IDs are
/// reported before root eligibility, matching the engine's selector
/// ambiguity behavior. The caller still runs `plan_task_move` as the
/// authoritative snapshot validation and mutation pass.
pub(crate) fn resolve_movable(
    contents: &str,
    block_id: &str,
) -> Result<MovableRoot, Ineligible> {
    let tasks = scan_task_lines(contents);
    let matches: Vec<&ScannedTask> = tasks
        .iter()
        .filter(|task| {
            !task.in_fence_or_frontmatter
                && task.block_id.as_deref() == Some(block_id)
        })
        .collect();
    let task = match matches.as_slice() {
        [] => return Err(Ineligible::Missing),
        [task] => *task,
        many => return Err(Ineligible::Ambiguous { count: many.len() }),
    };

    if !task.is_column0() {
        let parent = tasks
            .iter()
            .filter(|candidate| {
                candidate.line_index < task.line_index
                    && !candidate.in_fence_or_frontmatter
                    && candidate.indent < task.indent
            })
            .last();
        return Err(Ineligible::Nested {
            parent_id: parent.and_then(|parent| parent.block_id.clone()),
            parent_title: parent
                .map(|parent| parent.title.clone())
                .unwrap_or_else(|| "its parent task".to_string()),
        });
    }
    if task.is_prj() {
        return Err(Ineligible::Project);
    }
    if task.is_closed() {
        return Err(Ineligible::Closed {
            status: task.symbol,
        });
    }
    if !task.is_open_checkbox() {
        return Err(Ineligible::UnsupportedCheckbox {
            status: task.symbol,
        });
    }

    Ok(MovableRoot {
        line: task.line_index + 1,
        title: task.title.clone(),
        block_id: task.block_id.clone(),
        status: task.symbol,
        disabled_reason: None,
    })
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
    RestoreNoSource {
        path: PathBuf,
        detail: String,
    },
    RestoreRecurring {
        candidate: TaskCandidate,
    },
    RestoreTombstone {
        candidate: TaskCandidate,
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
    InvalidCaptureSlots {
        detail: String,
    },
    BlockIdCollision {
        candidate: TaskCandidate,
        path: PathBuf,
        block_id: String,
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
            Self::RestoreNoSource { path, detail } => {
                write!(
                    formatter,
                    "cannot restore from {}: {detail}",
                    path.display()
                )
            }
            Self::RestoreRecurring { candidate } => {
                write!(
                    formatter,
                    "refusing recurring task {}",
                    format_candidate(candidate)
                )
            }
            Self::RestoreTombstone { candidate } => {
                write!(
                    formatter,
                    "refusing merged-task tombstone {}",
                    format_candidate(candidate)
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
            Self::InvalidCaptureSlots { detail } => {
                write!(formatter, "invalid capture move slots: {detail}")
            }
            Self::BlockIdCollision {
                candidate,
                path,
                block_id,
            } => {
                write!(
                    formatter,
                    "refusing task {} because block ID ^{block_id} already exists in {}",
                    format_candidate(candidate),
                    path.display()
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

impl PlanError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::NoSelectors => "move_no_selectors",
            Self::InvalidPath { .. } => "move_invalid_path",
            Self::MissingNote { .. } => "move_missing_note",
            Self::AmbiguousNote { .. } => "move_ambiguous_note",
            Self::SameSourceAndDest { .. } => "move_same_note",
            Self::ArchivePath { .. } => "move_archive_path",
            Self::RestoreNoSource { .. } => "restore_no_source",
            Self::RestoreRecurring { .. } => "restore_recurring_task",
            Self::RestoreTombstone { .. } => "restore_merge_tombstone",
            Self::DestKind { .. } => "move_dest_kind",
            Self::ProjectWithoutTasks { .. } => "move_project_without_tasks",
            Self::SelectorZero { .. } => "move_selector_zero",
            Self::SelectorAmbiguous { .. } => "move_selector_ambiguous",
            Self::ClosedTask { .. } => "move_closed_task",
            Self::NestedTask { .. } => "move_nested_task",
            Self::ProjectTask { .. } => "move_project_task",
            Self::UnsupportedCheckbox { .. } => "move_unsupported_checkbox",
            Self::InvalidCaptureSlots { .. } => "move_invalid_capture_slots",
            Self::BlockIdCollision { .. } => "move_block_id_collision",
            Self::NotATask { .. } => "move_not_a_task",
            Self::DependsOnDestPrj { .. } => "move_depends_on_dest_prj",
            Self::DestBlocked { .. } => "move_dest_blocked",
            Self::AmbiguousIdentity { .. } => "move_ambiguous_identity",
            Self::NonCanonicalId { .. } => "move_non_canonical_id",
            Self::EncodedIdentityCollision { .. } => {
                "move_encoded_identity_collision"
            }
            Self::UnrepresentableIdentity { .. } => {
                "move_unrepresentable_identity"
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
    if request.restore && !is_archive_path(&source_path) {
        return Err(PlanError::RestoreNoSource {
            path: source_path,
            detail: "source is not under done/; restore takes an archive note"
                .to_string(),
        });
    }
    let dest_path = if request.restore && request.dest.trim().is_empty() {
        restore_bound_source(snapshot, &source_path)?
    } else {
        resolve_note(&note_index, &request.dest)?
    };
    if source_path == dest_path {
        return Err(PlanError::SameSourceAndDest { path: source_path });
    }
    if !request.restore {
        refuse_archive_path(&source_path, "source")?;
    }
    refuse_archive_path(&dest_path, "destination")?;

    let source_contents = snapshot_note(snapshot, &source_path)?;
    let dest_contents = snapshot_note(snapshot, &dest_path)?;
    let dest_kind = match classify_dest(&dest_path, dest_contents) {
        Ok(kind) => kind,
        Err(PlanError::DestKind { path, detail })
            if detail.contains("is a notebook and holds no tasks") =>
        {
            let owner = notebook_owner_from_snapshot(snapshot, &dest_path);
            return Err(PlanError::DestKind {
                path: path.clone(),
                detail: notebook_teaching_detail(&path, owner.as_deref()),
            });
        }
        Err(PlanError::DestKind { path, detail })
            if detail.contains("is an album and holds no tasks") =>
        {
            let owner = notebook_owner_from_snapshot(snapshot, &dest_path);
            let noun = album_noun_from_contents(dest_contents);
            return Err(PlanError::DestKind {
                path: path.clone(),
                detail: album_teaching_detail(
                    &path,
                    owner.as_deref(),
                    noun.as_deref(),
                ),
            });
        }
        Err(error) => return Err(error),
    };
    let dest_has_tasks = tasks_heading_index(dest_contents).is_some();
    if dest_kind == DestKind::Project && !dest_has_tasks {
        return Err(PlanError::ProjectWithoutTasks { path: dest_path });
    }

    let source_tasks = scan_task_lines(source_contents);
    let selected_with_selectors =
        select_tasks(source_contents, &source_tasks, &request.selectors)?;
    let (selected, capture_slots) = match &request.placement {
        MovePlacement::Append => (
            selected_with_selectors
                .into_iter()
                .map(|(_, task)| task)
                .collect::<Vec<_>>(),
            None,
        ),
        MovePlacement::CaptureSlots(slots) => {
            if slots.len() != request.selectors.len()
                || slots.len() != selected_with_selectors.len()
            {
                return Err(PlanError::InvalidCaptureSlots {
                    detail: format!(
                        "expected one slot per distinct selector ({}), got {}",
                        selected_with_selectors.len(),
                        slots.len()
                    ),
                });
            }
            let mut distinct_slots = BTreeSet::new();
            if slots.iter().any(|slot| !distinct_slots.insert(*slot)) {
                return Err(PlanError::InvalidCaptureSlots {
                    detail:
                        "each moved task must have a distinct root-task index"
                            .into(),
                });
            }
            let mut ordered = selected_with_selectors;
            ordered.sort_by_key(|(selector_index, _)| {
                (slots[*selector_index], *selector_index)
            });
            let ordered_slots = ordered
                .iter()
                .map(|(selector_index, _)| slots[*selector_index])
                .collect::<Vec<_>>();
            let selected = ordered
                .into_iter()
                .map(|(_, task)| task)
                .collect::<Vec<_>>();
            (selected, Some(ordered_slots))
        }
    };
    let dest_blocked = dest_prj_is_blocked(dest_contents);
    let source_lines: Vec<&str> =
        source_contents.split_inclusive('\n').collect();
    let source_logical: Vec<&str> = source_lines
        .iter()
        .map(|line| split_line_ending(line).0)
        .collect();
    let source_fenced =
        markdown::fenced_lines(&source_logical, 0..source_logical.len());
    let source_id_counts = count_live_block_ids(source_contents);

    for task in &selected {
        if request.restore {
            refuse_restore_selection(&source_logical, &source_fenced, task)?;
        }
        validate_movable_root(
            task,
            &source_path,
            &source_lines,
            &source_id_counts,
            request.allow_closed,
        )?;
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

    let mut blocks = Vec::new();
    for task in &selected {
        let end = task_block_end(&source_lines, task.line_index, 0);
        let fragment = transplant::cut_line_range(
            source_contents,
            &transplant::LineRange {
                start: task.line_index,
                end,
            },
        )
        .expect("selected task block is within the scanned source note");
        blocks.push(SelectedBlock {
            task: task.clone(),
            end,
            fragment,
        });
    }

    let raw_fragments: Vec<String> =
        blocks.iter().map(|block| block.fragment.clone()).collect();
    let dest_used: BTreeSet<String> = transplant::live_block_ids(dest_contents)
        .into_iter()
        .collect();
    let (fragments, renames, id_map) = transplant::rename_fragments(
        &raw_fragments,
        &dest_path,
        &dest_used,
        request.id_collision,
    )
    .map_err(|error| match error {
        transplant::RenameError::Collision {
            fragment_index,
            block_id,
        } => PlanError::BlockIdCollision {
            candidate: selected[fragment_index].candidate(),
            path: dest_path.clone(),
            block_id,
        },
    })?;
    let mut fragments: Vec<String> = fragments
        .iter()
        .map(|fragment| {
            transplant::rebase_fragment(fragment, "", "", "", "", false)
        })
        .collect();
    if request.reopen {
        fragments = fragments
            .iter()
            .map(|fragment| reopen_fragment_root(fragment))
            .collect();
    }
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

    let ranges: Vec<transplant::LineRange> = blocks
        .iter()
        .map(|block| transplant::LineRange {
            start: block.task.line_index,
            end: block.end,
        })
        .collect();
    let moved_old_ids: BTreeSet<String> = id_map.keys().cloned().collect();
    let stayed_source_ids: BTreeSet<String> = transplant::live_block_ids(
        &transplant::remove_line_ranges(source_contents, &ranges),
    )
    .into_iter()
    .filter(|id| !moved_old_ids.contains(id))
    .collect();

    let mut fragment_link_count = 0;
    let mut fragment_dep_count = 0;
    let mut repaired_fragments = Vec::new();
    for fragment in &fragments {
        let repaired = transplant::repair_moved_fragment(
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
    let source_without =
        transplant::remove_line_ranges(source_contents, &ranges);

    let (dest_with_insert, landing_lines) = insert_into_dest(
        &dest_existing_repair.contents,
        &repaired_fragments,
        dest_kind,
        capture_slots.as_deref(),
    )?;

    let mut repair_counts: BTreeMap<PathBuf, (usize, usize)> = BTreeMap::new();
    add_repair(
        &mut repair_counts,
        dest_path.clone(),
        dest_existing_repair.link_count + fragment_link_count,
        dest_existing_repair.dependency_metadata_count + fragment_dep_count,
    );
    let source_postimages =
        BTreeMap::from([(source_path.clone(), source_without)]);
    let repaired = transplant::repair_vault_postimages(
        &snapshot.notes,
        &source_postimages,
        &dest_path,
        dest_with_insert,
        &note_index,
        &moved_targets,
        repair_counts,
    );
    let transplanted = transplant::finish_transplant(
        repaired,
        repaired_fragments,
        renames,
        landing_lines,
    );
    let mut postimages = transplanted.postimages;
    if request.restore
        && let Some(source_postimage) = postimages.get(&source_path)
    {
        postimages.insert(
            source_path.clone(),
            trim_emptied_month_headings(source_postimage),
        );
    }

    let mut moves = Vec::new();
    let mut carried_block_tag = false;
    for ((task, _fragment), landing_line) in selected
        .iter()
        .zip(fragments.iter())
        .zip(transplanted.landing_lines.iter())
    {
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
        let dest_line = landing_line + 1;
        let destination_section = postimages
            .get(&dest_path)
            .and_then(|contents| section_for_line(contents, dest_line));
        moves.push(MoveRecord {
            source_path: source_path.clone(),
            dest_path: dest_path.clone(),
            source_line: task.line_index + 1,
            dest_line,
            title: task.title.clone(),
            old_block_id,
            new_block_id,
            old_dependency_id,
            new_dependency_id,
            destination_section,
            status_symbol: if request.reopen { ' ' } else { task.symbol },
        });
    }

    Ok(MovePlan {
        postimages,
        moves,
        renamed_block_ids: transplanted.renames,
        repaired_files: transplanted.repaired_files,
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

/// Resolve a restore destination to the archive's bound source through the
/// shared pairing resolver. Orphan and conflicted archives are refused
/// instead of guessed, so a restore never lands in the wrong note; pass an
/// explicit destination to override.
fn restore_bound_source(
    snapshot: &VaultSnapshot,
    archive: &Path,
) -> Result<PathBuf, PlanError> {
    let index = ArchivePairIndex::from_contents(&snapshot.notes);
    match index.for_archive(archive) {
        ArchiveLookup::Bound(bound) => Ok(bound.source),
        ArchiveLookup::Orphan { .. } => Err(PlanError::RestoreNoSource {
            path: archive.to_path_buf(),
            detail: "archive has no resolvable source; pass -d/--dest NOTE"
                .to_string(),
        }),
        ArchiveLookup::Refused { reason, related } => {
            let mut detail = format!(
                "archive binding is refused ({reason}); pass -d/--dest NOTE"
            );
            if !related.is_empty() {
                detail.push_str(&format!(
                    " (see {})",
                    related
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            Err(PlanError::RestoreNoSource {
                path: archive.to_path_buf(),
                detail,
            })
        }
    }
}

/// Restore-only selection refusals: recurring occurrences and merged-task
/// tombstones never come back (each needs explicit handling first). `^prj`
/// lines stay refused by the shared movable-task check.
fn refuse_restore_selection(
    lines: &[&str],
    fenced: &BTreeSet<usize>,
    task: &ScannedTask,
) -> Result<(), PlanError> {
    if is_recurring_task_line(&task.line) {
        return Err(PlanError::RestoreRecurring {
            candidate: task.candidate(),
        });
    }
    if task_block_has_merge_tombstone(lines, fenced, task.line_index) {
        return Err(PlanError::RestoreTombstone {
            candidate: task.candidate(),
        });
    }
    Ok(())
}

/// Whether the task block carries a merge tombstone: a direct-child LINKS
/// line with the canonical `merged into` relation entry that
/// `bob task merge` stamps on retired sources.
fn task_block_has_merge_tombstone(
    lines: &[&str],
    fenced: &BTreeSet<usize>,
    task_line: usize,
) -> bool {
    let merge_key = relation_label_key("merged into");
    task_relations::parse::links_children_of(lines, fenced, task_line)
        .iter()
        .any(|child| {
            let task_relations::LinksLine::Accepted { entries, .. } =
                &child.parsed
            else {
                return false;
            };
            entries.iter().any(|entry| {
                entry
                    .label
                    .as_ref()
                    .is_some_and(|label| relation_label_key(label) == merge_key)
            })
        })
}

/// Reopen one moved fragment's root line: checkbox back to `[ ]` with the
/// managed `[completion::]`/`[cancelled::]` close fields removed. Children
/// keep their bytes; only the root row is retouched.
fn reopen_fragment_root(fragment: &str) -> String {
    let mut pieces = fragment.split_inclusive('\n');
    let Some(first) = pieces.next() else {
        return fragment.to_string();
    };
    let (content, ending) = split_line_ending(first);
    let opened = set_task_line_status(content, ' ')
        .unwrap_or_else(|| content.to_string());
    let stripped = remove_cancelled_fields(&remove_completion_fields(&opened));
    let mut reopened = String::with_capacity(fragment.len());
    reopened.push_str(&stripped);
    reopened.push_str(ending);
    for rest in pieces {
        reopened.push_str(rest);
    }
    reopened
}

/// Remove managed `[cancelled:: DATE]` fields, mirroring
/// [`remove_completion_fields`]. Reopen is the only caller: a cancelled row
/// coming back open must not keep its cancel stamp.
fn remove_cancelled_fields(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut ranges = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = line[cursor..].find("[cancelled::") {
        let start = cursor + relative;
        let Some(close_relative) = line[start..].find(']') else {
            break;
        };
        let end = start + close_relative + 1;
        let mut remove_start = start;
        while remove_start > 0
            && matches!(bytes[remove_start - 1], b' ' | b'\t')
        {
            remove_start -= 1;
        }
        ranges.push((remove_start, end));
        cursor = end;
    }
    if ranges.is_empty() {
        return line.to_string();
    }
    let mut updated = line.to_string();
    for (start, end) in ranges.into_iter().rev() {
        updated.replace_range(start..end, "");
    }
    updated
}

/// Drop a keep-window month heading (`## YYYY-MM` or `## Undated`) that the
/// restore emptied: nothing but blank lines may remain between the heading
/// and the next heading or end of file. Fenced and frontmatter lines are
/// never headings, so archived code samples survive the trim.
fn trim_emptied_month_headings(contents: &str) -> String {
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();
    if lines.is_empty() {
        return contents.to_string();
    }
    let logical: Vec<&str> =
        lines.iter().map(|line| split_line_ending(line).0).collect();
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(&logical);
    let scan_start = frontmatter_end.map_or(0, |end| end + 1);
    let fenced = markdown::fenced_lines(&logical, scan_start..logical.len());
    let hidden: BTreeSet<usize> = frontmatter_end
        .map(|end| (0..=end).collect::<BTreeSet<usize>>())
        .unwrap_or_default()
        .union(&fenced)
        .copied()
        .collect();
    let mut drop_lines: BTreeSet<usize> = BTreeSet::new();
    for (index, line) in logical.iter().enumerate() {
        if hidden.contains(&index) || !is_month_heading(line) {
            continue;
        }
        let mut cursor = index + 1;
        while cursor < logical.len() && logical[cursor].trim().is_empty() {
            cursor += 1;
        }
        let emptied = cursor >= logical.len()
            || markdown::atx_heading(logical[cursor]).is_some();
        if !emptied {
            continue;
        }
        drop_lines.insert(index);
        for (offset, line) in logical[(index + 1)..cursor].iter().enumerate() {
            if line.trim().is_empty() {
                drop_lines.insert(index + 1 + offset);
            }
        }
    }
    if drop_lines.is_empty() {
        return contents.to_string();
    }
    lines
        .into_iter()
        .enumerate()
        .filter(|(index, _)| !drop_lines.contains(index))
        .map(|(_, line)| line)
        .collect()
}

/// Whether a logical line is a keep-window month heading: `## YYYY-MM` or
/// `## Undated`, the only section titles the archiver files closed roots
/// under.
fn is_month_heading(line: &str) -> bool {
    let Some(title) = line
        .strip_prefix("## ")
        .or_else(|| line.strip_prefix("##\t"))
        .map(str::trim)
    else {
        return false;
    };
    if title == "Undated" {
        return true;
    }
    let mut parts = title.split('-');
    matches!((parts.next(), parts.next(), parts.next()), (Some(year), Some(month), None) if year.len() == 4 && month.len() == 2 && year.bytes().all(|byte| byte.is_ascii_digit()) && month.bytes().all(|byte| byte.is_ascii_digit()))
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
    if frontmatter_is_notebook(&frontmatter) {
        return Err(PlanError::DestKind {
            path: path.to_path_buf(),
            detail: notebook_teaching_detail(path, None),
        });
    }
    if frontmatter_is_album(&frontmatter) {
        return Err(PlanError::DestKind {
            path: path.to_path_buf(),
            detail: album_teaching_detail(path, None, None),
        });
    }
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

fn notebook_teaching_detail(path: &Path, owner: Option<&str>) -> String {
    let display = path.to_string_lossy().replace('\\', "/");
    match owner {
        Some(route) => format!(
            "{display} is a notebook and holds no tasks — use its owner @{route}"
        ),
        None => format!(
            "{display} is a notebook and holds no tasks — choose an area or project and repair it with `bob notebooks check`"
        ),
    }
}

fn album_teaching_detail(
    path: &Path,
    owner: Option<&str>,
    noun: Option<&str>,
) -> String {
    let display = path.to_string_lossy().replace('\\', "/");
    let stem = display
        .rsplit('/')
        .next()
        .unwrap_or(&display)
        .strip_suffix(".md")
        .unwrap_or(&display);
    match (noun, owner) {
        (Some(noun), Some(route)) => format!(
            "{display} is an album and holds no tasks — @{stem}/ adds a {noun}; tasks go to @{route}"
        ),
        (Some(noun), None) => format!(
            "{display} is an album and holds no tasks — @{stem}/ adds a {noun}; choose an area or project for tasks"
        ),
        (None, Some(route)) => format!(
            "{display} is an album and holds no tasks — fix the album and use its owner @{route} for tasks"
        ),
        (None, None) => format!(
            "{display} is an album and holds no tasks — fix the album and choose an area or project for tasks"
        ),
    }
}

fn album_noun_from_contents(contents: &str) -> Option<String> {
    let head = contents.split_inclusive('\n').take(64).collect::<String>();
    // The snapshot path is unknown here; the declaration parser only needs
    // the relative file name for diagnostics, and the noun for teaching
    // copy comes from the parsed schema.
    crate::native::albums::parse_album_declaration(
        std::path::Path::new("album.md"),
        &head,
    )
    .and_then(|declaration| declaration.record_type)
    .map(|record_type| record_type.id)
}

fn notebook_owner_from_snapshot(
    snapshot: &VaultSnapshot,
    dest_path: &Path,
) -> Option<String> {
    let index = NoteIndex::from_paths(snapshot.notes.keys().cloned());
    let buffers: Vec<(String, String)> = snapshot
        .notes
        .iter()
        .map(|(path, contents)| {
            (path.to_string_lossy().replace('\\', "/"), contents.clone())
        })
        .collect();
    let source = IndexedHomeSource::from_buffers(&index, &buffers);
    let mut resolver = HomeResolver::new(source);
    let home = resolver.resolve_note(dest_path);
    if home.key == "@unfiled" || home.key == "@unlinked" {
        return None;
    }
    let stem = Path::new(&home.key)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_string)
        .unwrap_or(home.key.clone());
    if stem.is_empty() || stem.starts_with('@') {
        return None;
    }
    Some(stem)
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
) -> Result<Vec<(usize, ScannedTask)>, PlanError> {
    let visible: Vec<&ScannedTask> = tasks
        .iter()
        .filter(|task| !task.in_fence_or_frontmatter)
        .collect();
    let mut selected: BTreeMap<usize, (usize, ScannedTask)> = BTreeMap::new();
    for (selector_index, selector) in selectors.iter().enumerate() {
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
            MoveSelector::Line(index) => visible
                .iter()
                .copied()
                .filter(|task| task.line_index == *index)
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
        selected
            .entry(unique.line_index)
            .or_insert((selector_index, unique.clone()));
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

fn validate_movable_task(
    task: &ScannedTask,
    allow_closed: bool,
) -> Result<(), PlanError> {
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
        if !allow_closed {
            return Err(PlanError::ClosedTask {
                candidate: task.candidate(),
            });
        }
        // Capture routing moves completions home; closed symbols are
        // movable under the closed-selection option.
        return Ok(());
    }
    if !task.is_open_checkbox() {
        return Err(PlanError::UnsupportedCheckbox {
            candidate: task.candidate(),
        });
    }
    Ok(())
}

fn validate_movable_root(
    task: &ScannedTask,
    source_path: &Path,
    source_lines: &[&str],
    source_id_counts: &BTreeMap<String, usize>,
    allow_closed: bool,
) -> Result<(), PlanError> {
    validate_movable_task(task, allow_closed)?;
    let end = task_block_end(source_lines, task.line_index, 0);
    let fragment = source_lines[task.line_index..end].concat();
    validate_fragment_identities(&fragment, source_path, source_id_counts, task)
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
    transplant::live_block_ids(contents)
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

fn insert_into_dest(
    dest_contents: &str,
    fragments: &[String],
    dest_kind: DestKind,
    capture_slots: Option<&[usize]>,
) -> Result<(String, Vec<usize>), PlanError> {
    if let Some(slots) = capture_slots {
        if dest_kind != DestKind::Project {
            return Err(PlanError::InvalidCaptureSlots {
                detail: "slot placement requires a project destination".into(),
            });
        }
        if slots.len() != fragments.len() {
            return Err(PlanError::InvalidCaptureSlots {
                detail: format!(
                    "expected {} slots for {} moved tasks, got {}",
                    fragments.len(),
                    fragments.len(),
                    slots.len()
                ),
            });
        }
        let existing_roots = task_roots_in_tasks_lane(dest_contents);
        let final_root_count = existing_roots.len() + fragments.len();
        if let Some(slot) = slots.iter().find(|slot| **slot >= final_root_count)
        {
            return Err(PlanError::InvalidCaptureSlots {
                detail: format!(
                    "slot {slot} is outside the final lane of {final_root_count} root tasks"
                ),
            });
        }
        let mut updated = dest_contents.to_string();
        let mut landings = vec![0; fragments.len()];
        for (index, (slot, fragment)) in slots.iter().zip(fragments).enumerate()
        {
            let insert_at = task_slot_byte(&updated, *slot)?;
            let extra_separator =
                last_line_is_task_lane_marker(&updated[..insert_at]);
            let (next, landing_lines) = transplant::insert_fragments_at_byte(
                &updated,
                insert_at,
                std::slice::from_ref(fragment),
                extra_separator,
            )
            .ok_or_else(|| PlanError::InvalidCaptureSlots {
                detail: format!(
                    "slot {slot} produced an invalid insertion offset"
                ),
            })?;
            let line = landing_lines
                .first()
                .copied()
                .expect("one inserted task has one landing line");
            let inserted_lines = fragment.lines().count().max(1);
            for landing in landings.iter_mut().take(index) {
                if *landing >= line {
                    *landing += inserted_lines;
                }
            }
            landings[index] = line;
            updated = next;
        }
        return Ok((updated, landings));
    }

    if let Some(insert_at) = tasks_intake_insert_byte(dest_contents) {
        let extra_separator =
            last_line_is_task_lane_marker(&dest_contents[..insert_at]);
        let (updated, landings) = transplant::insert_fragments_at_byte(
            dest_contents,
            insert_at,
            fragments,
            extra_separator,
        )
        .expect("tasks intake insertion offset is a character boundary");
        return Ok((updated, landings));
    }
    if dest_kind == DestKind::Area {
        let insertion = transplant::join_fragments(
            fragments,
            transplant::document_line_ending(dest_contents),
        );
        let result = append_tasks_section(dest_contents, &insertion);
        let prefix_len = result.len().saturating_sub(insertion.len());
        let first_line = result[..prefix_len].matches('\n').count();
        return Ok((
            result,
            transplant::fragment_landing_lines(first_line, fragments),
        ));
    }
    Err(PlanError::ProjectWithoutTasks {
        path: PathBuf::from("destination"),
    })
}

fn task_roots_in_tasks_lane(contents: &str) -> Vec<usize> {
    let Some(heading) = tasks_heading_index(contents) else {
        return Vec::new();
    };
    let Some(end_byte) = tasks_intake_insert_byte(contents) else {
        return Vec::new();
    };
    let end_line = contents[..end_byte].matches('\n').count();
    scan_task_lines(contents)
        .into_iter()
        .filter(|task| {
            !task.in_fence_or_frontmatter
                && task.is_column0()
                && task.line_index > heading
                && task.line_index < end_line
        })
        .map(|task| task.line_index)
        .collect()
}

fn task_slot_byte(contents: &str, slot: usize) -> Result<usize, PlanError> {
    let roots = task_roots_in_tasks_lane(contents);
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();
    let line = roots.get(slot).copied().or_else(|| {
        (slot == roots.len())
            .then(|| tasks_intake_insert_byte(contents))
            .flatten()
            .map(|byte| contents[..byte].matches('\n').count())
    });
    let Some(line) = line else {
        return Err(PlanError::InvalidCaptureSlots {
            detail: format!("slot {slot} has no task row in the destination"),
        });
    };
    Ok(lines.iter().take(line).map(|line| line.len()).sum())
}

fn append_tasks_section(contents: &str, insertion: &str) -> String {
    let ending = transplant::document_line_ending(contents);
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

fn section_for_line(contents: &str, line_1based: usize) -> Option<String> {
    let lines: Vec<&str> = contents.lines().collect();
    let target = line_1based.saturating_sub(1);
    let mut section: Option<String> = None;
    for (index, line) in lines.iter().enumerate() {
        if index >= target {
            break;
        }
        if let Some((level, title)) = markdown::atx_heading(line)
            && level == 2
        {
            section = Some(title.to_string());
        }
    }
    section
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

fn last_line_is_task_lane_marker(prefix: &str) -> bool {
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
    task_status_groups::is_badge_row(last)
        || task_status_groups::is_legacy_badge_marker(last)
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

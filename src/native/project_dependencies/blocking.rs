//! Project recognition and effective-blocking reasons
//! (`docs/task-dependencies.md` §13.1, §13.3).
//!
//! Pure model shared by reconcile, close, and capture: which notes
//! count as projects, which tasks live in them, which `depends_on`
//! targets are open, and why a task is Blocked. Nothing here writes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use super::super::projects::{
    frontmatter_is_project, frontmatter_value, parse_frontmatter,
    project_schedule_date, ProjectStatus,
};
use super::super::task_dependencies::{dependency_id, is_archive_path};
use super::super::task_status_hooks::{
    canonical_daily_date, fenced_lines, logical_lines, task_id_openness,
    FileScan, TaskLine,
};
use super::super::vault_links::NoteIndex;
use super::parse::{parse_depends_on, DependsOnItem, DependsOnProperty};

/// Block id of the project completion-criteria task.
const PRJ_BLOCK_ID: &str = "prj";

/// A note's project shape: `project(T)` without the reasons
/// (`docs/task-dependencies.md` §13.3).
#[derive(Debug, Clone)]
pub(crate) struct ProjectContext {
    /// Vault-relative note path.
    pub(crate) relative_path: PathBuf,
    /// Whether the note is `project(T)`: `type: project`,
    /// non-terminal status, exactly one open `^prj`, and not a
    /// canonical daily note.
    pub(crate) recognised: bool,
    /// Project `scheduled` frontmatter date, if exactly one
    /// well-formed key exists.
    pub(crate) scheduled: Option<NaiveDate>,
    /// Parsed `depends_on` property (kept even when malformed so
    /// callers can warn).
    pub(crate) depends_on: DependsOnProperty,
    /// Fenced logical-line indices: fenced `#task` lines parse as
    /// tasks but are never residents (§13.1).
    fenced: BTreeSet<usize>,
}

impl ProjectContext {
    /// Build the project context for one scanned note.
    pub(crate) fn for_note(file: &FileScan) -> Self {
        let lines = logical_lines(&file.contents);
        let mut context = Self {
            relative_path: file.relative_path.clone(),
            recognised: false,
            scheduled: None,
            depends_on: DependsOnProperty::Absent,
            fenced: fenced_lines(&lines, 0..lines.len()),
        };
        let Some(frontmatter) = parse_frontmatter(&file.contents) else {
            return context;
        };
        if !frontmatter_is_project(&frontmatter) {
            return context;
        }
        if ProjectStatus::parse(frontmatter_value(&frontmatter, "status"))
            .is_terminal()
        {
            return context;
        }
        if canonical_daily_date(&file.relative_path).is_some() {
            return context;
        }
        if file
            .tasks
            .iter()
            .filter(|task| is_open_prj_task(task, &context.fenced))
            .count()
            != 1
        {
            return context;
        }
        context.recognised = true;
        context.scheduled =
            project_schedule_date(&file.relative_path, &frontmatter);
        context.depends_on = parse_depends_on(&frontmatter);
        context
    }

    /// Whether `task` is a resident: a recognised open `#task` line
    /// physically in the note except `^prj`, including nested and
    /// `#ref` tasks; fenced, blockquoted, and custom-status lines are
    /// excluded (§13.1). Blockquoted lines never parse, so only the
    /// fence set needs checking here.
    pub(crate) fn is_resident(&self, task: &TaskLine) -> bool {
        task.status_recognized
            && task.status_type.is_open()
            && task.block_id.as_deref() != Some(PRJ_BLOCK_ID)
            && !self.fenced.contains(&task.line_index)
    }

    /// Whether the project's reasons attach to `task`: `^prj` and
    /// residents alike (§13.3: the rule applies to `^prj` too).
    /// Openness-independent; callers decide what a closed task needs.
    pub(crate) fn covers(&self, task: &TaskLine) -> bool {
        self.recognised
            && task.status_recognized
            && !self.fenced.contains(&task.line_index)
    }
}

/// Whether `task` is an open `^prj` completion-criteria task.
fn is_open_prj_task(task: &TaskLine, fenced: &BTreeSet<usize>) -> bool {
    task.block_id.as_deref() == Some(PRJ_BLOCK_ID)
        && task.status_recognized
        && task.status_type.is_open()
        && !fenced.contains(&task.line_index)
}

/// Vault-wide task openness: `[id::]` identities plus `(note,
/// block-id)` locations. A `depends_on` target is open when its
/// resolved task is open by location, falling back to its `[id::]`.
#[derive(Debug, Clone, Default)]
pub(crate) struct Openness {
    /// `[id::]` → open, split out of `task_dependency_states`.
    pub(crate) ids: BTreeMap<String, bool>,
    /// `(note, block-id)` → open for resolved-target lookup.
    pub(crate) locations: BTreeMap<(PathBuf, String), bool>,
}

impl Openness {
    /// Build both openness maps over one scan.
    pub(crate) fn for_files(files: &[FileScan]) -> Self {
        let ids = task_id_openness(files);
        let mut locations = BTreeMap::new();
        for file in files {
            for task in &file.tasks {
                let Some(block_id) = &task.block_id else {
                    continue;
                };
                let open = locations
                    .entry((file.relative_path.clone(), block_id.clone()))
                    .or_insert(false);
                *open |= task.status_recognized && task.status_type.is_open();
            }
        }
        Self { ids, locations }
    }

    /// Openness of one `(note, block-id)` target: the location when
    /// the resolved note carries that block, else its `[id::]`.
    fn target_open(&self, path: &Path, block_id: &str) -> bool {
        if let Some(open) = self
            .locations
            .get(&(path.to_path_buf(), block_id.to_string()))
        {
            return *open;
        }
        dependency_id(path, block_id)
            .ok()
            .and_then(|id| self.ids.get(&id).copied())
            .unwrap_or(false)
    }
}

/// One resolved `depends_on` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedDependency {
    /// Canonical link text (`[[target#^block]]`).
    pub(crate) link: String,
    /// Resolved note path, if the target resolved.
    pub(crate) path: Option<PathBuf>,
    /// Same-note target: kept in YAML, never projected, never
    /// blocking (§13.6).
    pub(crate) internal: bool,
    /// Archive target: closed history, never blocking.
    pub(crate) archive: bool,
    /// Whether the target is open (and hence blocking).
    pub(crate) open: bool,
}

/// Resolve parsed entries against the vault: internal targets stay
/// internal, unresolved targets never block (R4 parity), and archive
/// targets are closed.
pub(crate) fn resolve_dependencies(
    items: &[DependsOnItem],
    project_path: &Path,
    index: &NoteIndex,
    openness: &Openness,
) -> Vec<ResolvedDependency> {
    items
        .iter()
        .map(|item| {
            let unresolved = || ResolvedDependency {
                link: item.link.clone(),
                path: None,
                internal: item.internal,
                archive: false,
                open: false,
            };
            if item.internal {
                return ResolvedDependency {
                    path: Some(project_path.to_path_buf()),
                    internal: true,
                    ..unresolved()
                };
            }
            let Some(path) = index.resolve(Some(project_path), &item.target)
            else {
                return unresolved();
            };
            if path == project_path {
                return ResolvedDependency {
                    path: Some(path),
                    internal: true,
                    ..unresolved()
                };
            }
            if is_archive_path(&path) {
                return ResolvedDependency {
                    path: Some(path),
                    archive: true,
                    ..unresolved()
                };
            }
            let open = openness.target_open(&path, &item.block_id);
            ResolvedDependency {
                link: item.link.clone(),
                path: Some(path),
                internal: false,
                archive: false,
                open,
            }
        })
        .collect()
}

/// A project's own blocking inputs (§13.3).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProjectBlockReasons {
    /// Open `depends_on` targets (internal targets excluded).
    pub(crate) open_dependencies: Vec<ResolvedDependency>,
    /// Project `scheduled` date when strictly later than today.
    pub(crate) future_scheduled: Option<NaiveDate>,
}

impl ProjectBlockReasons {
    /// Whether the project contributes no reason.
    pub(crate) fn is_empty(&self) -> bool {
        self.open_dependencies.is_empty() && self.future_scheduled.is_none()
    }
}

/// Pure project reasons for one recognised project: the open
/// `depends_on` targets plus a future project `scheduled`. A
/// malformed property contributes no known target (it warns and
/// skips destructive edits instead).
pub(crate) fn project_block_reasons(
    context: &ProjectContext,
    index: &NoteIndex,
    today: NaiveDate,
    openness: &Openness,
) -> ProjectBlockReasons {
    if !context.recognised {
        return ProjectBlockReasons::default();
    }
    let open_dependencies = match &context.depends_on {
        DependsOnProperty::Items(items) => {
            resolve_dependencies(items, &context.relative_path, index, openness)
                .into_iter()
                .filter(|resolved| resolved.open)
                .collect()
        }
        DependsOnProperty::Absent | DependsOnProperty::Empty => Vec::new(),
        DependsOnProperty::Malformed(_) => Vec::new(),
    };
    ProjectBlockReasons {
        open_dependencies,
        future_scheduled: context.scheduled.filter(|date| *date > today),
    }
}

/// Why a task is Blocked, with the §13.8 reason kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BlockReason {
    /// The task's own Depends-On line has an open prerequisite.
    LocalDependency { open_ids: Vec<String> },
    /// The task's own `scheduled` is after today.
    LocalSchedule { date: NaiveDate },
    /// The task's project has open `depends_on` targets.
    ProjectDependency {
        project: PathBuf,
        open_links: Vec<String>,
    },
    /// The task's project `scheduled` is after today.
    ProjectSchedule { project: PathBuf, date: NaiveDate },
}

impl BlockReason {
    /// The §13.8 reason kind for reconcile, capture, and plugin JSON.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::LocalDependency { .. } => "local_dependency",
            Self::LocalSchedule { .. } => "local_schedule",
            Self::ProjectDependency { .. } => "project_dependency",
            Self::ProjectSchedule { .. } => "project_schedule",
        }
    }
}

/// Pure effective-blocking reasons for one task (§13.3): its own open
/// prerequisites and future schedule, plus its project's reasons
/// when the project covers it. A task recovers only when every
/// reason here is gone.
pub(crate) fn effective_block_reasons(
    task: &TaskLine,
    local_open_dependency_ids: &[String],
    context: &ProjectContext,
    project_reasons: &ProjectBlockReasons,
    today: NaiveDate,
) -> Vec<BlockReason> {
    let mut reasons = Vec::new();
    if !local_open_dependency_ids.is_empty() {
        let mut open_ids = local_open_dependency_ids.to_vec();
        open_ids.sort();
        open_ids.dedup();
        reasons.push(BlockReason::LocalDependency { open_ids });
    }
    if let Some(date) = task.scheduled
        && date > today
    {
        reasons.push(BlockReason::LocalSchedule { date });
    }
    if context.covers(task) {
        if !project_reasons.open_dependencies.is_empty() {
            reasons.push(BlockReason::ProjectDependency {
                project: context.relative_path.clone(),
                open_links: project_reasons
                    .open_dependencies
                    .iter()
                    .map(|resolved| resolved.link.clone())
                    .collect(),
            });
        }
        if let Some(date) = project_reasons.future_scheduled {
            reasons.push(BlockReason::ProjectSchedule {
                project: context.relative_path.clone(),
                date,
            });
        }
    }
    reasons
}

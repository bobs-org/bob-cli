//! Pure successor-offer planner (`docs/task-dependencies.md` §12).
//!
//! One definition, two engines: this is the Rust half of the shared rule.
//! Given the completed set C of one capture gesture, the staged post-close
//! snapshot, the day text before the gesture, and a [`SuccessorSelection`],
//! it classifies each dependent as still-blocked, recover-only, or a keyed
//! offer. Picked offers mint, stamp freshness, and insert successor links;
//! unpicked offers recover with `not_linked: not_selected`. It never calls
//! `plan_task_link`, never retires a schedule, and never auto-links.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use chrono::NaiveDate;
use sha2::{Digest, Sha256};

use super::super::capture_task_toggle::{
    select_implicit_open_entry, select_typed_implicit_entry, TypedImplicit,
    TypedImplicitOutcome,
};
use super::super::freshness::placement::{stamp_fresh, UnblockScope};
use super::super::projects::edits::normalize_task_hide_tag;
use super::super::projects::{project_surface, ProjectSurface};
use super::super::{
    capture_block_ids::mint_block_id,
    capture_pomodoros,
    capture_task_toggle::set_task_line_status,
    collect_done::block_ids_in_markdown,
    config,
    note_tasks::NoteTaskSettings,
    note_tasks::{clean_description, stable_task_key},
    pomodoro::pomodoros_section_range,
    pomodoro_homes::{DiskHomeSource, HomeResolver},
    pomodoro_themes::{self, routing, Catalog},
    project_dependencies::{
        blocking_open_dependency_ids, is_project_task_id, ProjectBlockReasons,
    },
    projects::edits::normalize_prj_is_blocked_tag,
    task_dependencies::format::canonical_link,
    task_status_hooks::{
        logical_lines, scan_pomodoros, task_dependency_states, FileScan,
        TaskLine, TasksSettings,
    },
    vault_links::NoteIndex,
};
use super::project_close::{
    apply_recovery_writes, classify_project_members, project_states_for_files,
    project_still_blocks, recovered_unblocked_project_files, unblock_scope_for,
    ProjectResidentKind,
};
use super::project_pull_in::{
    classify_pull_in, detect_pull_ins, emit_rows, pull_in_hide_removed,
    pull_in_preview_line, pull_in_recover_status, PullInCandidate, PullInClass,
    RowEmit,
};
use crate::native::capture_dependency_tasks::{
    has_hide_tag, has_prj_is_blocked_tag,
};

/// Which offers [`plan_successors`] should pick. Unknown keys are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum SuccessorSelection {
    /// Pick nothing: every offer recovers with `not_selected`.
    #[default]
    None,
    /// Pick every offer.
    All,
    /// Pick offers whose keys are in the set.
    Keys(BTreeSet<String>),
}

/// Draft-list sort key: group, anchor (entry, bullet), note path, task line.
pub(crate) type OfferOrderKey = (u8, (usize, usize), PathBuf, usize);

/// Offer group (`docs/task-dependencies.md` §12.2 ORDER).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum OfferGroup {
    HandOff,
    Project,
    ProjectFinished,
}

impl OfferGroup {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::HandOff => "hand_off",
            Self::Project => "project",
            Self::ProjectFinished => "project_finished",
        }
    }
}

/// Where a picked offer's successor bullet lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntryRole {
    Slot,
    Closing,
    Implicit,
    Created,
    Typed,
}

impl EntryRole {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Slot => "slot",
            Self::Closing => "closing",
            Self::Implicit => "implicit",
            Self::Created => "created",
            Self::Typed => "typed",
        }
    }
}

/// Destination reported on an offer row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OfferDestination {
    pub day_file: PathBuf,
    pub entry_name: String,
    pub entry_role: EntryRole,
    pub fallback: bool,
}

/// Offer draft attached to a verdict before row emission.
#[derive(Debug, Clone)]
pub(crate) struct OfferDraft {
    pub group: OfferGroup,
    pub project: Option<PathBuf>,
    pub destination: OfferDestination,
    pub target: SuccessorTarget,
    pub key: String,
    pub block_id_needed: bool,
    pub selected: bool,
    /// A typed default was refused by the strict cap or has no eligible
    /// destination. The offer remains visible for an explicit retarget.
    pub placement_refusal: Option<&'static str>,
    /// The default destination was changed by D7 type placement.
    pub placement_routed: bool,
    pub n: usize,
    pub order_key: OfferOrderKey,
}

/// Offer fields on an [`UnblockedRow`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OfferReport {
    pub key: String,
    pub n: usize,
    pub group: OfferGroup,
    pub project: Option<PathBuf>,
    pub destination: OfferDestination,
    pub selected: bool,
    pub block_id_needed: bool,
    /// Crate-private draft order; never a JSON field.
    pub order_key: OfferOrderKey,
}

/// Folded counts for one project whose `^prj` entered project offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectFold {
    pub project: PathBuf,
    pub hidden: usize,
    pub already_planned: usize,
    pub sub_projects: usize,
}

/// One completed predecessor in C: a task this gesture moved open →
/// Done, read from the staged post-close text.
#[derive(Debug, Clone)]
pub(crate) struct CompletedTask {
    pub relative_path: PathBuf,
    pub block_id: String,
    pub task_id: String,
    pub text: String,
    pub is_root: bool,
}

/// Input to [`plan_successors`]: everything the rule reads, owned or
/// borrowed, so unit tests can drive the planner against in-memory
/// snapshots in the style of `recovery_tests.rs` (§11.6).
pub(crate) struct SuccessorInput<'a> {
    /// Vault root for resolving each successor's effective task type.
    /// Pure planner callers may omit it to keep legacy placement vectors
    /// independent of the host filesystem.
    pub bob_dir: Option<&'a Path>,
    pub completed: Vec<CompletedTask>,
    pub snapshot: Vec<(PathBuf, String)>,
    /// Staged post-gesture texts of C's notes for project pull-in trigger
    /// (a) detection (`docs/task-dependencies.md` §12.10 Gate). Empty keeps
    /// today's behaviour; `cli-capture` wires the planner texts.
    pub note_texts: Vec<(PathBuf, String)>,
    pub pre_day: Option<String>,
    /// Post-gesture day text before successor insertion. `None` for `!`
    /// (the pre-gesture text is the base); closes pass the post-close
    /// staged text.
    pub post_day: Option<String>,
    /// Closing-target mode for Pomodoro closes
    /// (`docs/task-dependencies.md` §12.3): every completed task anchors
    /// at the closed entry's continuation instead of a slot. `None` for
    /// `!`.
    pub closing: Option<ClosingContext>,
    pub day_relative: PathBuf,
    pub today: NaiveDate,
    pub tasks_settings: &'a TasksSettings,
    pub note_settings: &'a NoteTaskSettings,
    pub link_unblocked: bool,
    pub selection: SuccessorSelection,
    pub index: &'a NoteIndex,
}

/// Closing-target context for Pomodoro closes
/// (`docs/task-dependencies.md` §12.3 Placement).
#[derive(Debug, Clone)]
pub(crate) struct ClosingContext {
    /// Short entry name (`""` when the closed entry was unnamed).
    pub closed_name: String,
    /// 0-based line of the closed entry in the post-close day text.
    pub closed_line: usize,
    /// The continuation the close left behind, when it left one: its
    /// 1-based line plus whether the close created it.
    pub continuation: Option<ClosingContinuation>,
}

/// The continuation a close left behind for successor links.
#[derive(Debug, Clone)]
pub(crate) struct ClosingContinuation {
    /// 1-based line of the continuation entry in the post-close text.
    pub line: usize,
    /// True when the close created it (case 1); a reused same-name entry
    /// reports false.
    pub created: bool,
}

/// One predecessor behind an unblocked row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PredecessorRef {
    pub note_path: PathBuf,
    pub block_id: String,
    pub text: String,
}

/// Placement report for one linked successor: 1-based lines in the day
/// text after this gesture, for the JSON `link` object (§12.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SuccessorLinkReport {
    pub entry_name: String,
    pub entry_line: usize,
    pub line: usize,
    pub block_link: String,
    pub entry_created: bool,
    pub next_up: bool,
    pub block_id_created: bool,
}

/// One dependent this gesture changed or linked (§12.5 `unblocked[]`).
/// The row reports only when the gesture changed the status or linked
/// the dependent; `text` is the raw task description and the caller
/// cleans it for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnblockedRow {
    pub note_path: PathBuf,
    pub block_id: String,
    pub line: usize,
    pub text: String,
    pub previous_status_symbol: char,
    pub status_symbol: char,
    pub inbox: bool,
    pub unblocked_by: Vec<PredecessorRef>,
    pub link: Option<SuccessorLinkReport>,
    pub not_linked: Option<&'static str>,
    /// `"dependency"` for existing rows, `"project_empty"` for pull-ins.
    pub trigger: &'static str,
    /// Whether the gesture removed `#hide` from this row's line.
    pub hide_removed: bool,
    /// Present on every offered row, picked or not.
    pub offer: Option<OfferReport>,
}

/// One open dependent of C that stays blocked (§12.5 `still_blocked[]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StillBlockedRow {
    pub note_path: PathBuf,
    pub block_id: String,
    pub line: usize,
    pub text: String,
    pub status_symbol: char,
    pub reason: &'static str,
    pub waits_on: usize,
    pub scheduled: Option<String>,
    /// Vault-relative project note when `reason` is `project`.
    pub project: Option<PathBuf>,
    /// `"dependency"` for existing rows, `"project_empty"` for pull-ins.
    pub trigger: &'static str,
}

/// Output of [`plan_successors`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SuccessorPlan {
    /// False when the `[id::]` gate stopped the planner: no dependents
    /// lookup ran and both row lists are empty.
    pub ran_lookup: bool,
    pub unblocked: Vec<UnblockedRow>,
    pub still_blocked: Vec<StillBlockedRow>,
    /// Post-images for changed note files only, keyed by relative path.
    pub changed_files: BTreeMap<PathBuf, String>,
    /// Day text after successor insertion (pre-retirement), when at
    /// least one successor linked.
    pub new_day_text: Option<String>,
    /// One record per project whose `^prj` entered project offers.
    pub project_folds: Vec<ProjectFold>,
}

/// Whether a vault-relative note is an inbox note, mirroring nav's
/// `255-inbox-route.js::classifyInboxNote`: root `inbox.md`, plus direct
/// area children whose live frontmatter `parent` resolves to it. A
/// project or untyped child never counts, whatever its parent; neither
/// does a nested grandchild (its parent points at the area, not the
/// inbox). `contents` is the note's live (staged) text and `index` the
/// vault note index; only this candidate's frontmatter is read.
pub(crate) fn is_inbox_note(
    relative: &Path,
    contents: &str,
    index: &NoteIndex,
) -> bool {
    if normalized_relative(relative) == "inbox.md" {
        return true;
    }
    if !index
        .relative_paths()
        .any(|path| normalized_relative(path) == "inbox.md")
    {
        return false;
    }
    let Some(frontmatter) = super::super::projects::parse_frontmatter(contents)
    else {
        return false;
    };
    // nav's `getChildNoteInfo`: projects win over areas, and untyped
    // notes are plain — only areas can be inbox children.
    if super::super::projects::frontmatter_is_project(&frontmatter)
        || !super::super::projects::frontmatter_is_area(&frontmatter)
    {
        return false;
    }
    let Some(parent) = parent_field_text(contents) else {
        return false;
    };
    parent_link_targets(&parent).iter().any(|target| {
        index
            .resolve(Some(relative), target)
            .is_some_and(|resolved| {
                normalized_relative(&resolved) == "inbox.md"
            })
    })
}

/// The raw text of a note's frontmatter `parent` field: the scalar rest
/// of the `parent:` line, or the following `- item` lines when the rest
/// is empty (block-list form). Scanned only inside the `---` block so a
/// body `parent:` line never counts. Same key matching as the shared
/// `frontmatter_value` convention.
fn parent_field_text(contents: &str) -> Option<String> {
    let mut lines = contents.lines();
    let first = lines.next()?;
    if first.strip_suffix('\r').unwrap_or(first) != "---" {
        return None;
    }
    let mut collected: Option<String> = None;
    let mut block_items: Vec<String> = Vec::new();
    let mut in_block = false;
    for line in lines {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line == "---" {
            break;
        }
        if in_block {
            let trimmed = line.trim_start();
            if trimmed.is_empty() {
                continue;
            }
            if let Some(item) = trimmed
                .strip_prefix("- ")
                .or_else(|| trimmed.strip_prefix('-'))
            {
                block_items.push(item.trim().to_string());
                continue;
            }
            break;
        }
        let Some(rest) = line.strip_prefix("parent") else {
            continue;
        };
        let Some(value) = rest.strip_prefix(':') else {
            continue;
        };
        if !value.trim().is_empty() {
            collected = Some(value.trim().to_string());
            break;
        }
        in_block = true;
    }
    if in_block {
        if block_items.is_empty() {
            return None;
        }
        return Some(block_items.join("\n"));
    }
    collected.filter(|text| !text.is_empty())
}

/// Vault-relative path with native separators normalized, matching nav's
/// `normalizeVaultRelativePath` separator folding (case is significant,
/// exactly as nav's `=== "inbox.md"` check).
fn normalized_relative(relative: &Path) -> String {
    relative
        .components()
        .filter_map(|component| {
            component.as_os_str().to_str().map(str::to_string)
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Every link target a frontmatter `parent` value names: each `[[target]]`
/// (alias and `#section` stripped), falling back to the bare value itself
/// when it names no wikilink (mirroring nav's plain-text link fallback).
/// Flow (`[a, b]`) and block-list (`- a`) forms contribute each item.
fn parent_link_targets(value: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut items = Vec::new();
    let trimmed = value.trim();
    if trimmed.starts_with('[') && !trimmed.starts_with("[[") {
        let inner = trimmed
            .strip_prefix('[')
            .unwrap_or(trimmed)
            .strip_suffix(']')
            .unwrap_or(trimmed);
        for item in inner.split(',') {
            items.push(item.trim().trim_matches(['"', '\'']).to_string());
        }
    } else {
        for line in value.lines() {
            let line = line.trim();
            let item = line
                .strip_prefix("- ")
                .or_else(|| line.strip_prefix('-'))
                .map(str::trim)
                .unwrap_or(line);
            if !item.is_empty() {
                items.push(item.trim().trim_matches(['"', '\'']).to_string());
            }
        }
        if items.is_empty() && !trimmed.is_empty() {
            items.push(trimmed.trim_matches(['"', '\'']).to_string());
        }
    }
    for item in items {
        let mut found = false;
        let mut rest = item.as_str();
        while let Some(start) = rest.find("[[") {
            let after = &rest[start + 2..];
            let Some(end) = after.find("]]") else {
                break;
            };
            let target = after[..end]
                .split('|')
                .next()
                .unwrap_or_default()
                .split('#')
                .next()
                .unwrap_or_default()
                .trim();
            if !target.is_empty() {
                targets.push(target.to_string());
                found = true;
            }
            rest = &after[end + 2..];
        }
        if !found && !item.trim().is_empty() {
            targets.push(item.trim().to_string());
        }
    }
    targets
}

/// Whether a vault-relative note path is archive history: anything under
/// the top-level `done/` directory. Successor candidates never come
/// from there.
fn in_done_archive(relative: &Path) -> bool {
    matches!(
        relative.components().next(),
        Some(Component::Normal(name)) if name == "done"
    )
}

/// One ledger entry in the pre-gesture day text: its 0-based line,
/// short name (`""` when unnamed), and whether it is still open.
#[derive(Debug, Clone)]
pub(crate) struct DayEntry {
    line: usize,
    pub(crate) name: String,
    open: bool,
}

/// Slot anchor: insert after an anchor bullet's subtree.
#[derive(Debug, Clone)]
pub(crate) struct SlotAnchor {
    pub(crate) entry_index: usize,
    bullet_end: usize,
    indent: String,
}

/// The pre-gesture day text parsed once: open entries plus the first
/// live link per completed identity, in ledger order.
#[derive(Debug, Clone)]
pub(crate) struct DayView {
    pub(crate) entries: Vec<DayEntry>,
    pub(crate) live: BTreeMap<(PathBuf, String), (usize, usize, usize, String)>,
}

impl DayView {
    fn parse(text: &str, day_relative: &Path, index: &NoteIndex) -> Self {
        let lines = logical_lines(text);
        let Some(section) = pomodoros_section_range(&lines) else {
            return Self {
                entries: Vec::new(),
                live: BTreeMap::new(),
            };
        };
        let model = scan_pomodoros(&lines, section);
        let capture = capture_pomodoros::scan(text);
        let mut names: BTreeMap<usize, String> = BTreeMap::new();
        for entry in &capture.entries {
            names.insert(
                entry.line.saturating_sub(1),
                entry.name.clone().unwrap_or_default(),
            );
        }
        let entries = model
            .entries
            .iter()
            .map(|entry| DayEntry {
                line: entry.line_index,
                name: names.get(&entry.line_index).cloned().unwrap_or_default(),
                open: entry.open,
            })
            .collect::<Vec<_>>();
        let mut live = BTreeMap::new();
        for bullet in &model.bullets {
            let Some(entry) = model.entries.get(bullet.entry_index) else {
                continue;
            };
            if !entry.open {
                continue;
            }
            for link in &bullet.links {
                if link.struck {
                    continue;
                }
                let Some(path) =
                    index.resolve(Some(day_relative), &link.reference.target)
                else {
                    continue;
                };
                live.entry((path, link.reference.block_id.clone()))
                    .or_insert((
                        bullet.entry_index,
                        bullet.line_index,
                        bullet.end_line,
                        bullet.indentation.clone(),
                    ));
            }
        }
        Self { entries, live }
    }

    fn empty() -> Self {
        Self {
            entries: Vec::new(),
            live: BTreeMap::new(),
        }
    }

    /// Slot anchor for one completed task: its own first live link, else
    /// the root's anchor for a closed subtask (`inherit`), else none.
    fn anchor_for(
        &self,
        completed: &[CompletedTask],
        position: usize,
    ) -> Option<SlotAnchor> {
        let task = &completed[position];
        if let Some((entry_index, _bullet_line, bullet_end, indent)) = self
            .live
            .get(&(task.relative_path.clone(), task.block_id.clone()))
        {
            return Some(SlotAnchor::at(*entry_index, *bullet_end, indent));
        }
        if !task.is_root
            && let Some(root) = completed.iter().find(|task| task.is_root)
            && let Some((entry_index, _bullet_line, bullet_end, indent)) = self
                .live
                .get(&(root.relative_path.clone(), root.block_id.clone()))
        {
            return Some(SlotAnchor::at(*entry_index, *bullet_end, indent));
        }
        None
    }
}

impl SlotAnchor {
    fn at(entry_index: usize, bullet_end: usize, indent: &str) -> Self {
        Self {
            entry_index,
            bullet_end,
            indent: indent.to_string(),
        }
    }
}

/// Link form for a successor (`docs/task-dependencies.md` §12.4): the
/// canonical dependency-link form, except a successor living in the day
/// file itself still names the note (`[[20261009#^id]]`), never the bare
/// `[[#^id]]`.
pub(crate) fn successor_link_text(
    target: &Path,
    block_id: &str,
    day_relative: &Path,
    index: &NoteIndex,
) -> String {
    if target == day_relative {
        let stem = target
            .file_stem()
            .and_then(OsStr::to_str)
            .unwrap_or("daily");
        return format!("[[{stem}#^{block_id}]]");
    }
    canonical_link(target, block_id, day_relative, index)
}

/// One dependent's verdict before row filtering.
#[derive(Debug, Clone)]
pub(crate) struct DependentVerdict {
    pub(crate) file: usize,
    pub(crate) task: usize,
    pub(crate) preds: Vec<usize>,
    pub(crate) anchor: Option<SlotAnchor>,
    pub(crate) kind: VerdictKind,
    pub(crate) open_count: usize,
    /// True for §12.10 pull-in verdicts: `#hide` removal, surfaced rows,
    /// and the `[?]`-gated cascade.
    pub(crate) pull_in: bool,
    /// Offer draft when this row is selectable. Survives unpicked
    /// recovery as `selected: false`.
    pub(crate) offer: Option<OfferDraft>,
    /// Project note for a muted still-blocked resident (PDC3 / SO9).
    pub(crate) muted_project: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VerdictKind {
    StillBlockedWaitsOn,
    StillBlockedScheduled,
    StillBlockedProject,
    Recover(&'static str),
    Successor,
    Offer,
}

/// Run the shared successor rule (§12.2) over one gesture.
pub(crate) fn plan_successors(input: SuccessorInput) -> SuccessorPlan {
    plan_successors_inner(input, SuccessorMode::Completion)
}

/// Run only the existing dependency/project recovery rules for a
/// cancellation. This mode never detects project-finished pull-ins, builds
/// offers, assigns keys, selects rows, mints IDs, or writes successor links.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn plan_recovery_only(
    mut input: SuccessorInput,
    cancelled_project_paths: &BTreeSet<PathBuf>,
) -> SuccessorPlan {
    input.link_unblocked = false;
    input.selection = SuccessorSelection::None;
    plan_successors_inner(
        input,
        SuccessorMode::CancelRecovery {
            cancelled_project_paths: cancelled_project_paths.clone(),
        },
    )
}

#[derive(Debug, Clone)]
#[cfg_attr(not(test), allow(dead_code))]
enum SuccessorMode {
    Completion,
    CancelRecovery {
        cancelled_project_paths: BTreeSet<PathBuf>,
    },
}

impl SuccessorMode {
    fn is_cancel_recovery(&self) -> bool {
        matches!(self, Self::CancelRecovery { .. })
    }

    fn cancelled_project_paths(&self) -> &BTreeSet<PathBuf> {
        match self {
            Self::Completion => empty_paths(),
            Self::CancelRecovery {
                cancelled_project_paths,
            } => cancelled_project_paths,
        }
    }
}

fn empty_paths() -> &'static BTreeSet<PathBuf> {
    static EMPTY: std::sync::OnceLock<BTreeSet<PathBuf>> =
        std::sync::OnceLock::new();
    EMPTY.get_or_init(BTreeSet::new)
}

/// Build a read-only project-context view for a just-cancelled `^prj`.
/// Project membership normally requires its anchor task to be open, so
/// reopen only that parsed row in the planning snapshot; its terminal
/// postimage is still used for all returned writes.
fn prepare_cancelled_project_contexts(
    files: &mut [FileScan],
    cancelled_project_paths: &BTreeSet<PathBuf>,
) {
    for file in files {
        if !cancelled_project_paths.contains(&file.relative_path) {
            continue;
        }
        let Some(task_index) = file.tasks.iter().position(|task| {
            task.block_id
                .as_deref()
                .is_some_and(|id| is_project_task_id(Some(id)))
        }) else {
            continue;
        };
        let line_index = file.tasks[task_index].line_index;
        let Some(raw) = file.contents.lines().nth(line_index) else {
            continue;
        };
        let Some(reopened) = set_task_line_status(raw, ' ') else {
            continue;
        };
        file.contents =
            replace_logical_line(&file.contents, line_index, &reopened);
        let task = &mut file.tasks[task_index];
        task.status = ' ';
        task.status_type =
            super::super::task_status_hooks::TaskStatusType::Todo;
        task.status_recognized = true;
    }
}

fn replace_logical_line(
    content: &str,
    index: usize,
    replacement: &str,
) -> String {
    let mut output = String::with_capacity(content.len() + replacement.len());
    for (line_index, segment) in content.split_inclusive('\n').enumerate() {
        if line_index != index {
            output.push_str(segment);
            continue;
        }
        let ending = if segment.ends_with("\r\n") {
            "\r\n"
        } else if segment.ends_with('\n') {
            "\n"
        } else {
            ""
        };
        output.push_str(replacement);
        output.push_str(ending);
    }
    output
}

fn plan_successors_inner(
    input: SuccessorInput,
    mode: SuccessorMode,
) -> SuccessorPlan {
    // Merge the staged texts of C's notes (§12.10 Gate) over the snapshot:
    // a project note without `dependsOn`/`id::` markers is never in the
    // dependents snapshot, even when staged.
    let mut snapshot = input.snapshot.clone();
    for (relative_path, contents) in &input.note_texts {
        if let Some(entry) =
            snapshot.iter_mut().find(|(path, _)| path == relative_path)
        {
            entry.1 = contents.clone();
        } else {
            snapshot.push((relative_path.clone(), contents.clone()));
        }
    }
    let mut files = Vec::new();
    for (relative_path, contents) in &snapshot {
        files.push(FileScan {
            path: relative_path.clone(),
            relative_path: relative_path.clone(),
            contents: contents.clone(),
            tasks: super::super::task_status_hooks::parse_tasks(
                contents,
                input.tasks_settings,
            ),
        });
    }
    // Surfaces for pull-in detection over the staged texts.
    let surfaces: Vec<Option<ProjectSurface>> = files
        .iter()
        .map(|file| {
            project_surface(&file.relative_path, &file.contents, input.today)
        })
        .collect();
    let staged_notes: BTreeSet<PathBuf> = input
        .note_texts
        .iter()
        .map(|(path, _)| path.clone())
        .collect();
    let pull_ins = if mode.is_cancel_recovery() {
        Vec::new()
    } else {
        detect_pull_ins(
            &input.completed,
            &files,
            &surfaces,
            &staged_notes,
            Some(input.index),
            input.today,
        )
    };
    // GATE: without an `[id::]` on any completed task no dependent can
    // name this close, so skip the lookup with no extra reads — unless a
    // pull-in candidate was detected from the staged texts.
    if !input.completed.iter().any(|task| !task.task_id.is_empty())
        && pull_ins.is_empty()
        && mode.cancelled_project_paths().is_empty()
    {
        return SuccessorPlan::default();
    }
    let states = task_dependency_states(&files, &BTreeSet::new());
    let mut project_context_files = files.clone();
    if mode.is_cancel_recovery() {
        prepare_cancelled_project_contexts(
            &mut project_context_files,
            mode.cancelled_project_paths(),
        );
    }
    let mut project_states = project_states_for_files(
        &project_context_files,
        &states,
        input.today,
        input.index,
    );
    if mode.is_cancel_recovery() {
        for (index, file) in files.iter().enumerate() {
            if mode.cancelled_project_paths().contains(&file.relative_path)
                && let Some(state) = project_states.get_mut(index)
            {
                // The project task itself is now terminal, so its former
                // prerequisite/date reasons no longer block its residents.
                // Keep the inherited parent marker: cancellation never
                // crosses a subproject boundary.
                state.reasons = ProjectBlockReasons::default();
            }
        }
    }
    let day = match input.pre_day.as_deref() {
        Some(text) => DayView::parse(text, &input.day_relative, input.index),
        None => DayView::empty(),
    };
    // Closing anchors (§12.3): a close completes links out of its own
    // entry, so every predecessor anchors at the same continuation. One
    // shared dummy keeps the ordering on note path and line.
    let anchors = if input.closing.is_some() {
        input
            .completed
            .iter()
            .map(|_| Some(SlotAnchor::at(usize::MAX, usize::MAX, "\t")))
            .collect::<Vec<_>>()
    } else {
        input
            .completed
            .iter()
            .enumerate()
            .map(|(position, _)| day.anchor_for(&input.completed, position))
            .collect::<Vec<_>>()
    };

    // Classify every open direct dependent of C.
    let pull_in_by_task: BTreeMap<(usize, usize), &PullInCandidate> = pull_ins
        .iter()
        .map(|candidate| ((candidate.file, candidate.task), candidate))
        .collect();
    let mut verdicts = Vec::new();
    for (file_index, file) in files.iter().enumerate() {
        if in_done_archive(&file.relative_path) {
            continue;
        }
        for (task_index, task) in file.tasks.iter().enumerate() {
            if !task.status_recognized || !task.status_type.is_open() {
                continue;
            }
            let mut preds = Vec::new();
            for (position, completed) in input.completed.iter().enumerate() {
                if completed.task_id.is_empty() {
                    continue;
                }
                if task.depends_on.contains(&completed.task_id) {
                    preds.push(position);
                }
            }
            if let Some(candidate) =
                pull_in_by_task.get(&(file_index, task_index))
            {
                // §12.10 pull-in: one row per R; the pull-in replaces R's
                // SL11 `project_task` / `hidden` row.
                let open_count = blocking_open_dependency_ids(
                    file,
                    task,
                    states
                        .get(&(file_index, task_index))
                        .map(|state| state.open_dependency_ids.as_slice())
                        .unwrap_or(&[]),
                )
                .len();
                let surface = surfaces[file_index]
                    .as_ref()
                    .expect("pull-in candidate has a surface");
                let live_before = task.block_id.as_ref().is_some_and(|id| {
                    day.live
                        .contains_key(&(file.relative_path.clone(), id.clone()))
                });
                let inherited = project_states
                    .get(file_index)
                    .and_then(|state| state.inherited.clone());
                let class = classify_pull_in(
                    surface,
                    open_count,
                    live_before,
                    earliest_anchor(&candidate.preds, &anchors),
                    input.link_unblocked,
                    input.today,
                    inherited,
                );
                let (kind, anchor) = match class {
                    PullInClass::StillBlockedWaitsOn => {
                        (VerdictKind::StillBlockedWaitsOn, None)
                    }
                    PullInClass::StillBlockedScheduled => {
                        (VerdictKind::StillBlockedScheduled, None)
                    }
                    PullInClass::StillBlockedProject => {
                        (VerdictKind::StillBlockedProject, None)
                    }
                    PullInClass::Recover(reason) => {
                        (VerdictKind::Recover(reason), None)
                    }
                    PullInClass::Offer { anchor } => {
                        (VerdictKind::Offer, anchor)
                    }
                };
                verdicts.push(DependentVerdict {
                    file: file_index,
                    task: task_index,
                    preds: candidate.preds.clone(),
                    anchor,
                    kind,
                    open_count,
                    pull_in: true,
                    offer: None,
                    muted_project: None,
                });
                continue;
            }
            if preds.is_empty() {
                continue;
            }
            let open_count = blocking_open_dependency_ids(
                file,
                task,
                states
                    .get(&(file_index, task_index))
                    .map(|state| state.open_dependency_ids.as_slice())
                    .unwrap_or(&[]),
            )
            .len();
            let kind = if open_count > 0 {
                VerdictKind::StillBlockedWaitsOn
            } else if task.scheduled.is_some_and(|date| date > input.today) {
                VerdictKind::StillBlockedScheduled
            } else if project_still_blocks(file_index, task, &project_states)
                .is_some()
            {
                // PDC1 (§13.16): own reasons gone, project reasons remain.
                VerdictKind::StillBlockedProject
            } else if task
                .block_id
                .as_deref()
                .is_some_and(|id| is_project_task_id(Some(id)))
            {
                // PDC5 (§13.16): a tagged `^prj` inherits its block from
                // an ancestor, so losing its last own prerequisite
                // leaves it `[?]` with no `project_task` recover and no
                // PDC2/PDC3 for its residents.
                if has_prj_is_blocked_tag(&raw_task_line(
                    &file.contents,
                    task.line_index,
                )) {
                    VerdictKind::StillBlockedProject
                } else {
                    VerdictKind::Recover("project_task")
                }
            } else if has_hide_tag(&raw_task_line(
                &file.contents,
                task.line_index,
            )) {
                VerdictKind::Recover("hidden")
            } else if task.block_id.as_ref().is_some_and(|id| {
                day.live
                    .contains_key(&(file.relative_path.clone(), id.clone()))
            }) {
                VerdictKind::Recover("already_planned")
            } else if !input.link_unblocked {
                VerdictKind::Recover("disabled")
            } else {
                VerdictKind::Offer
            };
            let anchor = match kind {
                VerdictKind::Offer => earliest_anchor(&preds, &anchors),
                _ => None,
            };
            verdicts.push(DependentVerdict {
                file: file_index,
                task: task_index,
                preds,
                anchor,
                kind,
                open_count,
                pull_in: false,
                offer: None,
                muted_project: None,
            });
        }
    }

    // A cancelled ^prj has no direct predecessor transition from which the
    // completion planner could infer a project recovery. Seed its existing
    // resident classifier explicitly, even if the ^prj has no [id::] field.
    if mode.is_cancel_recovery() {
        for (file_index, file) in files.iter().enumerate() {
            if !mode.cancelled_project_paths().contains(&file.relative_path) {
                continue;
            }
            if let Some(task_index) = file.tasks.iter().position(|task| {
                task.block_id
                    .as_deref()
                    .is_some_and(|id| is_project_task_id(Some(id)))
            }) {
                verdicts.push(DependentVerdict {
                    file: file_index,
                    task: task_index,
                    preds: Vec::new(),
                    anchor: None,
                    kind: VerdictKind::Recover("cancelled_project"),
                    open_count: 0,
                    pull_in: false,
                    offer: None,
                    muted_project: None,
                });
            }
        }
    }

    let insertion_base = input
        .post_day
        .as_deref()
        .or(input.pre_day.as_deref())
        .unwrap_or("");
    let implicit = resolve_implicit_destination(
        insertion_base,
        &input.day_relative,
        input.index,
    );

    if !mode.is_cancel_recovery() {
        attach_hand_off_and_pull_in_offers(
            &mut verdicts,
            &files,
            &day,
            &input.day_relative,
            input.closing.is_some(),
            implicit.as_ref(),
            input.note_settings,
        );
    }

    let members = classify_project_members(
        &verdicts,
        &files,
        &states,
        &project_states,
        input.today,
        &day.live,
    );
    let mut project_folds = Vec::new();
    apply_project_member_classes(
        &mut verdicts,
        &mut project_folds,
        members,
        &files,
        &day,
        &anchors,
        &surfaces,
        &input.day_relative,
        input.closing.is_some(),
        implicit.as_ref(),
        input.link_unblocked,
        input.note_settings,
    );

    if !mode.is_cancel_recovery()
        && let Some(root) = input.bob_dir
        && let Some(mut type_routing) =
            SuccessorTypeRouting::load(root, input.today)
    {
        let placement_day = input
            .post_day
            .as_deref()
            .or(input.pre_day.as_deref())
            .unwrap_or("");
        let closing_target = input.closing.as_ref().map(|closing| {
            resolve_closing_target(
                placement_day,
                &input.day_relative,
                input.index,
                closing,
            )
        });
        route_successor_offers(
            &mut verdicts,
            &files,
            &mut type_routing,
            placement_day,
            &input.day_relative,
            input.index,
            closing_target.as_ref(),
        );
    }

    if !mode.is_cancel_recovery() {
        assign_offer_keys(&mut verdicts, &files, input.note_settings);
        apply_selection(&mut verdicts, &input.selection);
        number_offers(&mut verdicts, &files);
    }

    let mut successor_positions = verdicts
        .iter()
        .enumerate()
        .filter(|(_, verdict)| verdict.kind == VerdictKind::Successor)
        .map(|(position, _)| position)
        .collect::<Vec<_>>();
    successor_positions.sort_by(|left, right| {
        successor_order_key(&verdicts[*left], &files)
            .cmp(&successor_order_key(&verdicts[*right], &files))
    });

    apply_verdicts(
        input,
        files,
        verdicts,
        successor_positions,
        day,
        &project_states,
        &surfaces,
        project_folds,
    )
}

/// One resolved closing target (`docs/task-dependencies.md` §12.3):
/// where every successor of a Pomodoro close goes, plus the entry name
/// the JSON rows report and whether the entry is new since the gesture.
pub(crate) struct ResolvedClosingTarget {
    pub(crate) target: SuccessorTarget,
    pub(crate) name: String,
    pub(crate) entry_created: bool,
}

/// Resolve the closing target against the post-close day text: the
/// continuation the close left behind, else the first open same-name
/// entry after the closed one, else a created placeholder immediately
/// after the closed entry's block (exactly where the close would have
/// created its continuation). Successors count as carried.
fn resolve_closing_target(
    text: &str,
    day_relative: &Path,
    index: &NoteIndex,
    closing: &ClosingContext,
) -> ResolvedClosingTarget {
    let day = DayView::parse(text, day_relative, index);
    let lines = logical_lines(text);
    let end = pomodoros_section_range(&lines)
        .map(|range| range.end)
        .unwrap_or(lines.len());
    let end = end.min(lines.len());
    let block_end = |headline: usize| -> usize {
        super::super::capture_pomodoros::pomodoro_block_range(
            &lines,
            headline.min(end),
            end,
        )
        .end
    };
    let position_of = |line: usize| -> Option<usize> {
        day.entries.iter().position(|entry| entry.line == line)
    };
    // Case 1: the continuation the close left behind. A close-created
    // continuation counts as entry-created: it did not exist before this
    // gesture.
    if let Some(continuation) = closing.continuation.as_ref()
        && let Some(position) = position_of(continuation.line.saturating_sub(1))
    {
        let entry = &day.entries[position];
        return ResolvedClosingTarget {
            target: SuccessorTarget::Closing {
                entry_index: Some(position),
                insert_at: block_end(entry.line),
                created_name: None,
            },
            name: entry.name.clone(),
            entry_created: continuation.created,
        };
    }
    // Case 2: the first open entry after the closed one with the same
    // non-empty name.
    let reused = (!closing.closed_name.is_empty())
        .then(|| position_of(closing.closed_line))
        .flatten()
        .and_then(|closed_position| {
            day.entries
                .iter()
                .enumerate()
                .skip(closed_position + 1)
                .find(|(_, entry)| {
                    entry.open && entry.name == closing.closed_name
                })
                .map(|(position, _)| position)
        });
    if let Some(position) = reused {
        let entry = &day.entries[position];
        return ResolvedClosingTarget {
            target: SuccessorTarget::Closing {
                entry_index: Some(position),
                insert_at: block_end(entry.line),
                created_name: None,
            },
            name: entry.name.clone(),
            entry_created: false,
        };
    }
    // Case 3: a created placeholder right after the closed entry's
    // block. The entry index stays `None` so the inserter writes the
    // `- [ ] () — NAME` headline itself.
    let insert_at = position_of(closing.closed_line)
        .map(|position| block_end(day.entries[position].line))
        .unwrap_or_else(|| block_end(closing.closed_line));
    ResolvedClosingTarget {
        target: SuccessorTarget::Closing {
            entry_index: None,
            insert_at,
            created_name: Some(closing.closed_name.clone()),
        },
        name: closing.closed_name.clone(),
        entry_created: true,
    }
}

/// Live-link keys of a day text: every unstruck Task Link bullet under
/// an open entry, as `(note path, block id)`. Used by the net batch
/// post-pass to verify a successor bullet still survives live
/// (`docs/task-dependencies.md` §12.5 SL20).
pub(crate) fn live_link_keys(
    day_text: &str,
    day_relative: &Path,
    index: &NoteIndex,
) -> BTreeSet<(PathBuf, String)> {
    DayView::parse(day_text, day_relative, index)
        .live
        .into_keys()
        .collect()
}

fn earliest_anchor(
    preds: &[usize],
    anchors: &[Option<SlotAnchor>],
) -> Option<SlotAnchor> {
    preds
        .iter()
        .filter_map(|position| anchors[*position].clone())
        .min_by_key(|anchor| (anchor.entry_index, anchor.bullet_end))
}

fn successor_order_key(
    verdict: &DependentVerdict,
    files: &[FileScan],
) -> ((usize, usize), PathBuf, usize) {
    let anchor_key = verdict
        .offer
        .as_ref()
        .map(|offer| match &offer.target {
            SuccessorTarget::Slot {
                entry_index,
                bullet_end,
                ..
            } => (*entry_index, *bullet_end),
            SuccessorTarget::Closing { entry_index, .. } => {
                (entry_index.unwrap_or(usize::MAX), 0)
            }
        })
        .or_else(|| {
            verdict
                .anchor
                .as_ref()
                .map(|anchor| (anchor.entry_index, anchor.bullet_end))
        })
        .unwrap_or((usize::MAX, usize::MAX));
    let file = &files[verdict.file];
    let task = &file.tasks[verdict.task];
    (anchor_key, file.relative_path.clone(), task.line_index)
}

fn offer_order_key(
    verdict: &DependentVerdict,
    files: &[FileScan],
) -> (u8, (usize, usize), PathBuf, usize) {
    let group = match verdict.offer.as_ref().map(|offer| offer.group) {
        Some(OfferGroup::HandOff) => 0,
        Some(OfferGroup::Project) => 1,
        Some(OfferGroup::ProjectFinished) => 2,
        None => 3,
    };
    let ((entry, bullet), path, line) = successor_order_key(verdict, files);
    (group, (entry, bullet), path, line)
}

struct ImplicitDest {
    target: SuccessorTarget,
    entry_name: String,
    entry_index: usize,
}

fn resolve_implicit_destination(
    base_text: &str,
    day_relative: &Path,
    index: &NoteIndex,
) -> Option<ImplicitDest> {
    let scan = capture_pomodoros::scan(base_text);
    let entry = select_implicit_open_entry(&scan).ok()?;
    let day = DayView::parse(base_text, day_relative, index);
    let line_0 = entry.line.saturating_sub(1);
    let entry_index =
        day.entries.iter().position(|item| item.line == line_0)?;
    let lines = logical_lines(base_text);
    let end = pomodoros_section_range(&lines)
        .map(|range| range.end.min(lines.len()))
        .unwrap_or(lines.len());
    let insert_at =
        capture_pomodoros::pomodoro_block_range(&lines, line_0.min(end), end)
            .end;
    Some(ImplicitDest {
        target: SuccessorTarget::Closing {
            entry_index: Some(entry_index),
            insert_at,
            created_name: None,
        },
        entry_name: entry.name.clone().unwrap_or_default(),
        entry_index,
    })
}

fn slot_target(anchor: &SlotAnchor) -> SuccessorTarget {
    SuccessorTarget::Slot {
        entry_index: anchor.entry_index,
        bullet_end: anchor.bullet_end,
        indent: anchor.indent.clone(),
    }
}

fn offer_destination(
    day_relative: &Path,
    entry_name: String,
    entry_role: EntryRole,
    fallback: bool,
) -> OfferDestination {
    OfferDestination {
        day_file: day_relative.to_path_buf(),
        entry_name,
        entry_role,
        fallback,
    }
}

fn slot_entry_name(day: &DayView, anchor: &SlotAnchor) -> String {
    day.entries
        .get(anchor.entry_index)
        .map(|entry| entry.name.clone())
        .unwrap_or_default()
}

fn make_offer(
    group: OfferGroup,
    project: Option<PathBuf>,
    destination: OfferDestination,
    target: SuccessorTarget,
    block_id_needed: bool,
) -> OfferDraft {
    OfferDraft {
        group,
        project,
        destination,
        target,
        key: String::new(),
        block_id_needed,
        selected: false,
        placement_refusal: None,
        placement_routed: false,
        n: 0,
        order_key: (3, (usize::MAX, usize::MAX), PathBuf::new(), 0),
    }
}

fn block_id_needed(task: &TaskLine) -> bool {
    task.block_id.is_none()
}

fn attach_offer(
    verdict: &mut DependentVerdict,
    files: &[FileScan],
    day: &DayView,
    day_relative: &Path,
    closing: bool,
    implicit: Option<&ImplicitDest>,
    group: OfferGroup,
    project: Option<PathBuf>,
) -> bool {
    let file = &files[verdict.file];
    let task = &file.tasks[verdict.task];
    let needed = block_id_needed(task);
    if closing {
        let dummy = SlotAnchor::at(usize::MAX, usize::MAX, "\t");
        let target = verdict
            .anchor
            .as_ref()
            .map(slot_target)
            .unwrap_or_else(|| slot_target(&dummy));
        verdict.kind = VerdictKind::Offer;
        verdict.offer = Some(make_offer(
            group,
            project,
            offer_destination(
                day_relative,
                String::new(),
                EntryRole::Closing,
                false,
            ),
            target,
            needed,
        ));
        return true;
    }
    if let Some(anchor) = verdict.anchor.as_ref() {
        let name = slot_entry_name(day, anchor);
        verdict.kind = VerdictKind::Offer;
        verdict.offer = Some(make_offer(
            group,
            project,
            offer_destination(day_relative, name, EntryRole::Slot, false),
            slot_target(anchor),
            needed,
        ));
        return true;
    }
    let Some(implicit) = implicit else {
        verdict.kind = VerdictKind::Recover("no_destination");
        verdict.anchor = None;
        verdict.offer = None;
        return false;
    };
    verdict.kind = VerdictKind::Offer;
    verdict.anchor = Some(SlotAnchor::at(implicit.entry_index, 0, "\t"));
    verdict.offer = Some(make_offer(
        group,
        project,
        offer_destination(
            day_relative,
            implicit.entry_name.clone(),
            EntryRole::Implicit,
            true,
        ),
        implicit.target.clone(),
        needed,
    ));
    true
}

fn attach_hand_off_and_pull_in_offers(
    verdicts: &mut [DependentVerdict],
    files: &[FileScan],
    day: &DayView,
    day_relative: &Path,
    closing: bool,
    implicit: Option<&ImplicitDest>,
    _note_settings: &NoteTaskSettings,
) {
    for verdict in verdicts.iter_mut() {
        if verdict.kind != VerdictKind::Offer {
            continue;
        }
        let group = if verdict.pull_in {
            OfferGroup::ProjectFinished
        } else {
            OfferGroup::HandOff
        };
        attach_offer(
            verdict,
            files,
            day,
            day_relative,
            closing,
            implicit,
            group,
            None,
        );
    }
}

fn apply_project_member_classes(
    verdicts: &mut Vec<DependentVerdict>,
    folds: &mut Vec<ProjectFold>,
    members: Vec<super::project_close::ProjectResidentClass>,
    files: &[FileScan],
    day: &DayView,
    anchors: &[Option<SlotAnchor>],
    surfaces: &[Option<ProjectSurface>],
    day_relative: &Path,
    closing: bool,
    implicit: Option<&ImplicitDest>,
    link_unblocked: bool,
    _note_settings: &NoteTaskSettings,
) {
    let mut grouped: BTreeMap<
        usize,
        Vec<super::project_close::ProjectResidentClass>,
    > = BTreeMap::new();
    for member in members {
        grouped.entry(member.file).or_default().push(member);
    }
    for (file_index, residents) in grouped {
        let mut hidden = 0usize;
        let mut already_planned = 0usize;
        let project_path = files[file_index].relative_path.clone();
        let sub_projects = surfaces
            .get(file_index)
            .and_then(|surface| surface.as_ref())
            .map(|surface| surface.open_subprojects)
            .unwrap_or(0);
        for resident in residents {
            let existing = verdicts.iter().position(|verdict| {
                verdict.file == resident.file && verdict.task == resident.task
            });
            if let Some(position) = existing {
                let kind = verdicts[position].kind;
                let is_hand_off =
                    verdicts[position].offer.as_ref().is_some_and(|offer| {
                        offer.group == OfferGroup::HandOff
                    }) || kind == VerdictKind::Offer
                        && !verdicts[position].pull_in;
                match kind {
                    VerdictKind::StillBlockedWaitsOn
                    | VerdictKind::StillBlockedScheduled
                    | VerdictKind::StillBlockedProject => continue,
                    VerdictKind::Recover("project_task") => continue,
                    _ if is_hand_off => continue,
                    _ => {}
                }
                match resident.kind {
                    ProjectResidentKind::StillBlockedWaitsOn => {
                        verdicts[position].kind =
                            VerdictKind::StillBlockedWaitsOn;
                        verdicts[position].open_count = resident.open_count;
                        verdicts[position].muted_project =
                            Some(project_path.clone());
                        verdicts[position].offer = None;
                    }
                    ProjectResidentKind::StillBlockedScheduled => {
                        verdicts[position].kind =
                            VerdictKind::StillBlockedScheduled;
                        verdicts[position].muted_project =
                            Some(project_path.clone());
                        verdicts[position].offer = None;
                    }
                    ProjectResidentKind::Hidden => {
                        hidden += 1;
                        verdicts[position].kind =
                            VerdictKind::Recover("hidden");
                        verdicts[position].anchor = None;
                        verdicts[position].offer = None;
                    }
                    ProjectResidentKind::AlreadyPlanned => {
                        already_planned += 1;
                        verdicts[position].kind =
                            VerdictKind::Recover("already_planned");
                        verdicts[position].anchor = None;
                        verdicts[position].offer = None;
                    }
                    ProjectResidentKind::Offerable => {
                        if !link_unblocked {
                            verdicts[position].kind =
                                VerdictKind::Recover("disabled");
                            verdicts[position].anchor = None;
                            verdicts[position].offer = None;
                            continue;
                        }
                        verdicts[position].preds = resident.preds.clone();
                        verdicts[position].anchor =
                            earliest_anchor(&resident.preds, anchors);
                        verdicts[position].kind = VerdictKind::Offer;
                        attach_offer(
                            &mut verdicts[position],
                            files,
                            day,
                            day_relative,
                            closing,
                            implicit,
                            OfferGroup::Project,
                            Some(project_path.clone()),
                        );
                    }
                }
                continue;
            }
            match resident.kind {
                ProjectResidentKind::Hidden => hidden += 1,
                ProjectResidentKind::AlreadyPlanned => already_planned += 1,
                _ => {}
            }
            let (kind, anchor, muted_project) = match resident.kind {
                ProjectResidentKind::StillBlockedWaitsOn => (
                    VerdictKind::StillBlockedWaitsOn,
                    None,
                    Some(project_path.clone()),
                ),
                ProjectResidentKind::StillBlockedScheduled => (
                    VerdictKind::StillBlockedScheduled,
                    None,
                    Some(project_path.clone()),
                ),
                ProjectResidentKind::Hidden => {
                    (VerdictKind::Recover("hidden"), None, None)
                }
                ProjectResidentKind::AlreadyPlanned => {
                    (VerdictKind::Recover("already_planned"), None, None)
                }
                ProjectResidentKind::Offerable if !link_unblocked => {
                    (VerdictKind::Recover("disabled"), None, None)
                }
                ProjectResidentKind::Offerable => (
                    VerdictKind::Offer,
                    earliest_anchor(&resident.preds, anchors),
                    None,
                ),
            };
            let mut verdict = DependentVerdict {
                file: resident.file,
                task: resident.task,
                preds: resident.preds,
                anchor,
                kind,
                open_count: resident.open_count,
                pull_in: false,
                offer: None,
                muted_project,
            };
            if kind == VerdictKind::Offer {
                attach_offer(
                    &mut verdict,
                    files,
                    day,
                    day_relative,
                    closing,
                    implicit,
                    OfferGroup::Project,
                    Some(project_path.clone()),
                );
            }
            verdicts.push(verdict);
        }
        folds.push(ProjectFold {
            project: project_path,
            hidden,
            already_planned,
            sub_projects,
        });
    }
}

fn offer_key_for(
    file: &FileScan,
    task: &TaskLine,
    digest_ordinal: usize,
    note_settings: &NoteTaskSettings,
) -> String {
    stable_task_key(
        &file.relative_path,
        task.block_id.as_deref(),
        &task.description,
        note_settings.global_filter.as_str(),
        digest_ordinal,
    )
}

fn assign_offer_keys(
    verdicts: &mut [DependentVerdict],
    files: &[FileScan],
    note_settings: &NoteTaskSettings,
) {
    let mut positions: Vec<usize> = verdicts
        .iter()
        .enumerate()
        .filter(|(_, verdict)| verdict.offer.is_some())
        .map(|(position, _)| position)
        .collect();
    positions.sort_by_key(|position| {
        let verdict = &verdicts[*position];
        (
            files[verdict.file].relative_path.clone(),
            files[verdict.file].tasks[verdict.task].line_index,
        )
    });
    let mut seen: BTreeMap<(PathBuf, String), usize> = BTreeMap::new();
    for position in positions {
        let verdict = &verdicts[position];
        let file = &files[verdict.file];
        let task = &file.tasks[verdict.task];
        let ordinal = if task.block_id.is_some() {
            1
        } else {
            let description = clean_description(
                &task.description,
                note_settings.global_filter.as_str(),
                None,
            );
            let digest = hex::encode(Sha256::digest(description.as_bytes()));
            let key = (file.relative_path.clone(), digest[..8].to_string());
            let count = seen.entry(key).or_insert(0);
            *count += 1;
            *count
        };
        let key = offer_key_for(file, task, ordinal, note_settings);
        if let Some(offer) = verdicts[position].offer.as_mut() {
            offer.key = key;
        }
    }
}

fn apply_selection(
    verdicts: &mut [DependentVerdict],
    selection: &SuccessorSelection,
) {
    for verdict in verdicts.iter_mut() {
        let Some(offer) = verdict.offer.as_mut() else {
            continue;
        };
        let picked = match selection {
            SuccessorSelection::All => true,
            SuccessorSelection::None => false,
            SuccessorSelection::Keys(keys) => keys.contains(&offer.key),
        };
        offer.selected = picked;
        if picked {
            if let Some(reason) = offer.placement_refusal {
                offer.selected = false;
                verdict.kind = VerdictKind::Recover(reason);
            } else {
                verdict.kind = VerdictKind::Successor;
            }
        } else {
            verdict.kind = VerdictKind::Recover("not_selected");
        }
    }
}

fn number_offers(verdicts: &mut [DependentVerdict], files: &[FileScan]) {
    let mut positions: Vec<usize> = verdicts
        .iter()
        .enumerate()
        .filter(|(_, verdict)| verdict.offer.is_some())
        .map(|(position, _)| position)
        .collect();
    positions.sort_by(|left, right| {
        offer_order_key(&verdicts[*left], files)
            .cmp(&offer_order_key(&verdicts[*right], files))
    });
    let keys: Vec<OfferOrderKey> = positions
        .iter()
        .map(|position| offer_order_key(&verdicts[*position], files))
        .collect();
    for (index, (position, key)) in positions.into_iter().zip(keys).enumerate()
    {
        if let Some(offer) = verdicts[position].offer.as_mut() {
            offer.n = index + 1;
            offer.order_key = key;
        }
    }
}

struct SuccessorTypeRouting {
    config: config::PlanConfig,
    catalog: Catalog,
    resolver: HomeResolver<DiskHomeSource>,
}

impl SuccessorTypeRouting {
    fn load(root: &Path, today: NaiveDate) -> Option<Self> {
        Some(Self {
            config: config::load_plan_config(&config::config_path()).ok()?,
            catalog: Catalog::load(root, today, false).ok()?,
            resolver: HomeResolver::new(DiskHomeSource::new(root)),
        })
    }

    fn suggestion(
        &mut self,
        path: &Path,
        line: &str,
    ) -> Option<pomodoro_themes::suggest::ThemeSuggestion> {
        pomodoro_themes::suggest::suggest_for_task(
            &mut self.resolver,
            &self.catalog,
            &self.config,
            path,
            line,
        )
        .suggestion
    }
}

/// Apply D7 to each offered successor's default destination. Existing
/// queues are left in their anchor slot when that entry agrees; a mismatch
/// uses the shared typed-placement planner.
fn route_successor_offers(
    verdicts: &mut [DependentVerdict],
    files: &[FileScan],
    type_routing: &mut SuccessorTypeRouting,
    day_text: &str,
    day_relative: &Path,
    index: &NoteIndex,
    closing_target: Option<&ResolvedClosingTarget>,
) {
    let day = DayView::parse(day_text, day_relative, index);
    let scan = capture_pomodoros::scan(day_text);
    let sources = type_routing.catalog.theme_sources();
    let exempt = sources.exempt.iter().cloned().collect::<Vec<_>>();
    for verdict in verdicts {
        let Some(offer) = verdict.offer.as_mut() else {
            continue;
        };
        let file = &files[verdict.file];
        let task = &file.tasks[verdict.task];
        let line = raw_task_line(&file.contents, task.line_index);
        let Some(suggestion) =
            type_routing.suggestion(&file.relative_path, &line)
        else {
            continue;
        };

        let default_target = closing_target
            .map(|resolved| resolved.target.clone())
            .unwrap_or_else(|| offer.target.clone());
        let default_name = closing_target
            .map(|resolved| resolved.name.clone())
            .unwrap_or_else(|| successor_target_name(&default_target, &day));
        offer.target = default_target.clone();
        offer.destination.entry_name = default_name.clone();
        if routing::agrees(
            Some(&suggestion.theme.key),
            Some(&default_name),
            &sources,
        ) {
            continue;
        }

        let typed = TypedImplicit {
            type_key: &suggestion.theme.key,
            type_name: &suggestion.theme.name,
            exempt: &exempt,
            cap: type_routing.config.max_themes(),
            strict: type_routing.config.strict(),
        };
        match select_typed_implicit_entry(&scan, &typed) {
            Ok(TypedImplicitOutcome::Entry(line)) => {
                let Some((entry_index, entry_name, target)) =
                    successor_entry_target(day_text, &day, line)
                else {
                    offer.placement_refusal = Some("no_destination");
                    offer.placement_routed = true;
                    continue;
                };
                if successor_target_entry_index(&default_target)
                    == Some(entry_index)
                {
                    continue;
                }
                offer.target = target;
                offer.destination.entry_name = entry_name;
                offer.destination.entry_role = EntryRole::Typed;
                offer.destination.fallback = false;
                offer.placement_routed = true;
            }
            Ok(TypedImplicitOutcome::Create { name }) => {
                offer.target = SuccessorTarget::Closing {
                    entry_index: None,
                    insert_at: named_creation_insert_at(day_text),
                    created_name: Some(name.clone()),
                };
                offer.destination.entry_name = name;
                offer.destination.entry_role = EntryRole::Created;
                offer.destination.fallback = false;
                offer.placement_routed = true;
            }
            Err(error) => {
                offer.placement_refusal = Some(match error {
                    super::super::capture_task_toggle::TypedImplicitError::StrictTypeCap { .. } => "strict_cap",
                    _ => "no_destination",
                });
                offer.destination.entry_name = suggestion.theme.name;
                offer.destination.entry_role = EntryRole::Created;
                offer.destination.fallback = false;
                offer.placement_routed = true;
            }
        }
    }
}

fn successor_target_entry_index(target: &SuccessorTarget) -> Option<usize> {
    match target {
        SuccessorTarget::Slot { entry_index, .. } => Some(*entry_index),
        SuccessorTarget::Closing { entry_index, .. } => *entry_index,
    }
}

fn successor_target_name(target: &SuccessorTarget, day: &DayView) -> String {
    if let Some(index) = successor_target_entry_index(target) {
        return day
            .entries
            .get(index)
            .map(|entry| entry.name.clone())
            .unwrap_or_default();
    }
    match target {
        SuccessorTarget::Closing {
            created_name: Some(name),
            ..
        } => name.clone(),
        _ => String::new(),
    }
}

fn successor_entry_target(
    day_text: &str,
    day: &DayView,
    line: usize,
) -> Option<(usize, String, SuccessorTarget)> {
    let entry_index =
        day.entries.iter().position(|entry| entry.line == line)?;
    let entry = day.entries.get(entry_index)?;
    let lines = logical_lines(day_text);
    let end = pomodoros_section_range(&lines)
        .map(|range| range.end.min(lines.len()))
        .unwrap_or(lines.len());
    let insert_at = capture_pomodoros::pomodoro_block_range(
        &lines,
        entry.line.min(end),
        end,
    )
    .end;
    Some((
        entry_index,
        entry.name.clone(),
        SuccessorTarget::Closing {
            entry_index: Some(entry_index),
            insert_at,
            created_name: None,
        },
    ))
}

/// Match the shared named-placeholder insertion point: after the running
/// entry or last completed entry, otherwise before the first open entry,
/// or at the top of `## Pomodoros` when the ledger is empty.
fn named_creation_insert_at(day_text: &str) -> usize {
    let lines = logical_lines(day_text);
    let end = pomodoros_section_range(&lines)
        .map(|range| range.end.min(lines.len()))
        .unwrap_or(lines.len());
    let scan = capture_pomodoros::scan(day_text);
    let anchor = scan
        .entries
        .iter()
        .find(|entry| {
            entry.state == capture_pomodoros::PomodoroState::Open
                && entry.time_range.is_some()
        })
        .or_else(|| {
            scan.entries.iter().rev().find(|entry| {
                entry.state == capture_pomodoros::PomodoroState::Completed
            })
        });
    if let Some(anchor) = anchor {
        return capture_pomodoros::pomodoro_block_range(
            &lines,
            anchor.line.saturating_sub(1).min(end),
            end,
        )
        .end;
    }
    scan.entries
        .first()
        .map(|entry| entry.line.saturating_sub(1))
        .or_else(|| pomodoros_section_range(&lines).map(|range| range.start))
        .unwrap_or(lines.len())
}

pub(crate) fn offer_report(offer: &OfferDraft) -> OfferReport {
    OfferReport {
        key: offer.key.clone(),
        n: offer.n,
        group: offer.group,
        project: offer.project.clone(),
        destination: offer.destination.clone(),
        selected: offer.selected,
        block_id_needed: offer.block_id_needed,
        order_key: offer.order_key.clone(),
    }
}

pub(crate) fn raw_task_line(contents: &str, line_index: usize) -> String {
    contents
        .lines()
        .nth(line_index)
        .unwrap_or_default()
        .trim_end_matches('\r')
        .to_string()
}

/// One successor placement for [`insert_successor_links`].
#[derive(Debug, Clone)]
pub(crate) struct SuccessorPlacement {
    pub target: SuccessorTarget,
    pub block_link: String,
}

/// Where a successor bullet goes: after an anchor bullet's subtree, or
/// into a closing target (the continuation the close created, the first
/// open same-name entry after the closed one, or a created placeholder).
/// Closing targets are built here so `capture_close` can wire them; only
/// slot targets occur for `!`.
#[derive(Debug, Clone)]
pub(crate) enum SuccessorTarget {
    Slot {
        entry_index: usize,
        bullet_end: usize,
        indent: String,
    },
    Closing {
        entry_index: Option<usize>,
        insert_at: usize,
        created_name: Option<String>,
    },
}

/// One placed successor bullet: 1-based lines in the day text after
/// insertion, for the JSON `link` object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlacedSuccessor {
    pub line: usize,
    pub entry_line: usize,
    pub entry_created: bool,
    pub next_up: bool,
}

/// Next rank for a linked successor (`docs/task-dependencies.md` §12.2):
/// ` ` and `?` become `*`; `*` and `/` are unchanged. A Pending `^prj` is
/// never demoted.
pub(crate) fn successor_new_status(previous: char) -> char {
    match previous {
        '?' | ' ' => '*',
        _ => previous,
    }
}

/// Mint new statuses and block IDs, insert successor links into the day
/// text, and build the row lists. Recovered ranks derive from the
/// post-insertion day text (what the hooks will see); retirement runs
/// after this and never moves a successor bullet for `!`, so the
/// pre-retirement projection is exact there.
#[allow(clippy::too_many_lines)]
fn apply_verdicts(
    input: SuccessorInput,
    files: Vec<FileScan>,
    verdicts: Vec<DependentVerdict>,
    successor_positions: Vec<usize>,
    day: DayView,
    project_states: &super::project_close::ProjectCloseStates,
    surfaces: &[Option<ProjectSurface>],
    project_folds: Vec<ProjectFold>,
) -> SuccessorPlan {
    let mut plan = SuccessorPlan {
        ran_lookup: true,
        project_folds,
        ..SuccessorPlan::default()
    };
    // Mint successor block IDs in placement order so the per-note used
    // set (staged IDs plus IDs minted earlier in this gesture) is
    // deterministic. A pulled-in `^prj` already has its block ID, so
    // nothing is ever minted for a pull-in.
    let mut minted: BTreeMap<(PathBuf, usize), String> = BTreeMap::new();
    let mut minted_in_note: BTreeMap<PathBuf, HashSet<String>> =
        BTreeMap::new();
    for position in &successor_positions {
        let verdict = &verdicts[*position];
        if verdict.pull_in
            || files[verdict.file].tasks[verdict.task]
                .block_id
                .as_deref()
                .is_some_and(|id| is_project_task_id(Some(id)))
        {
            continue;
        }
        let file = &files[verdict.file];
        let task = &file.tasks[verdict.task];
        if task.block_id.is_some() {
            continue;
        }
        let staged = &file.contents;
        let owned: HashSet<String> =
            block_ids_in_markdown(staged).into_iter().collect();
        let extra = minted_in_note
            .entry(file.relative_path.clone())
            .or_default();
        let mut used: HashSet<&str> =
            owned.iter().map(String::as_str).collect();
        used.extend(extra.iter().map(String::as_str));
        let description = clean_description(
            &task.description,
            input.note_settings.global_filter.as_str(),
            None,
        );
        let id = mint_block_id(&description, &used);
        extra.insert(id.clone());
        minted.insert((file.relative_path.clone(), task.line_index), id);
    }

    // Insertion base: the post-close text for closes, the pre-gesture
    // text for `!` (retirement runs after this and never moves a
    // successor bullet for `!`).
    let base_day_text = input
        .post_day
        .clone()
        .or_else(|| input.pre_day.clone())
        .unwrap_or_default();
    // One shared closing target for every successor of a close; slot
    // anchors stay per-successor for `!`.
    let closing_target = input.closing.as_ref().map(|closing| {
        resolve_closing_target(
            &base_day_text,
            &input.day_relative,
            input.index,
            closing,
        )
    });
    let mut verdicts = verdicts;
    if let Some(resolved) = closing_target.as_ref() {
        let role = if resolved.entry_created {
            EntryRole::Created
        } else {
            EntryRole::Closing
        };
        for verdict in verdicts.iter_mut() {
            if let Some(offer) = verdict.offer.as_mut()
                && !offer.placement_routed
            {
                offer.destination.entry_name = resolved.name.clone();
                offer.destination.entry_role = role;
                offer.target = resolved.target.clone();
            }
        }
    }
    // Build placements and insert them into the day text before
    // retirement (for `!`) or as the final day text (for closes).
    let mut placements = Vec::new();
    for position in &successor_positions {
        let verdict = &verdicts[*position];
        let file = &files[verdict.file];
        let task = &file.tasks[verdict.task];
        let block_id = task
            .block_id
            .clone()
            .or_else(|| {
                minted
                    .get(&(file.relative_path.clone(), task.line_index))
                    .cloned()
            })
            .unwrap_or_default();
        let block_link = successor_link_text(
            &file.relative_path,
            &block_id,
            &input.day_relative,
            input.index,
        );
        if let Some(offer) = verdict.offer.as_ref() {
            placements.push(SuccessorPlacement {
                target: offer.target.clone(),
                block_link,
            });
            continue;
        }
        if let Some(resolved) = closing_target.as_ref() {
            placements.push(SuccessorPlacement {
                target: resolved.target.clone(),
                block_link,
            });
            continue;
        }
        let anchor = verdict.anchor.as_ref().expect("successor has an anchor");
        placements.push(SuccessorPlacement {
            target: SuccessorTarget::Slot {
                entry_index: anchor.entry_index,
                bullet_end: anchor.bullet_end,
                indent: anchor.indent.clone(),
            },
            block_link,
        });
    }
    // Live-link baseline for derived recover ranks: insertions change
    // which bullets exist while status/mint edits never do, so parsing
    // the base with insertions (before status edits) is exact.
    let inserted_base = if placements.is_empty() {
        base_day_text.clone()
    } else {
        insert_successor_links(
            &base_day_text,
            &input.day_relative,
            input.index,
            placements.clone(),
        )
        .0
    };
    let post_live =
        DayView::parse(&inserted_base, &input.day_relative, input.index);

    // Apply note edits: successor statuses plus mints, then recovered
    // ranks with project-close writes. No line is added or removed
    // (in-place status swaps plus field and tag edits), so snapshot
    // line numbers stay valid.
    let mut edited: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut edit_line = |relative: &PathBuf,
                         line_index: usize,
                         new_status: Option<char>,
                         mint: Option<&str>,
                         recovery: Option<(NaiveDate, UnblockScope)>,
                         strip_tag: bool,
                         strip_hide: bool,
                         stamp: Option<NaiveDate>| {
        let staged = edited.get(relative).cloned().unwrap_or_else(|| {
            files
                .iter()
                .find(|file| &file.relative_path == relative)
                .map(|file| file.contents.clone())
                .unwrap_or_default()
        });
        let mut lines: Vec<String> =
            staged.split('\n').map(str::to_string).collect();
        if let Some(line) = lines.get_mut(line_index) {
            let trimmed_end =
                line.strip_suffix('\r').unwrap_or(line).to_string();
            let mut updated = match new_status {
                Some(status) => set_task_line_status(&trimmed_end, status)
                    .unwrap_or(trimmed_end.clone()),
                None => trimmed_end,
            };
            if let Some(id) = mint {
                updated.push_str(" ^");
                updated.push_str(id);
            }
            if strip_hide {
                // §12.10 step 4: `#hide` comes off before any
                // recover-rank write, through sync's own normalizer.
                updated = normalize_task_hide_tag(&updated, false, true);
            }
            if let Some((today, scope)) = recovery {
                // PDC2/UB1 (§13.16, freshness.md §10b): recovered rows
                // lose `#prj_is_blocked`, and Ready landings write
                // `[unblocked::]`. Successor links never reach here.
                let landed = new_status.unwrap_or(' ');
                updated =
                    apply_recovery_writes(&updated, today, landed, &scope);
            } else if strip_tag {
                updated = normalize_prj_is_blocked_tag(&updated, false);
            }
            if let Some(date) = stamp {
                updated = stamp_fresh(&updated, date).line;
            }
            if line.ends_with('\r') {
                updated.push('\r');
            }
            *line = updated;
        }
        edited.insert(relative.clone(), lines.join("\n"));
    };
    for position in &successor_positions {
        let verdict = &verdicts[*position];
        let file = &files[verdict.file];
        let task = &file.tasks[verdict.task];
        let mint = if verdict.pull_in {
            None
        } else {
            minted
                .get(&(file.relative_path.clone(), task.line_index))
                .map(String::as_str)
        };
        edit_line(
            &file.relative_path,
            task.line_index,
            Some(successor_new_status(task.status)),
            mint,
            None,
            true,
            verdict.pull_in,
            Some(input.today),
        );
    }
    // Recovered ranks derive from the post-gesture day text: `*` when the
    // dependent keeps a live link there, else ` `; only `?` moves.
    // Recovered rows also take the project-close writes (tag strip plus
    // `[unblocked::]` on Ready landings).
    for verdict in verdicts.iter() {
        let VerdictKind::Recover(reason) = verdict.kind else {
            continue;
        };
        let file = &files[verdict.file];
        let task = &file.tasks[verdict.task];
        if verdict.pull_in {
            // §12.10 steps 4–9 line writes: the status, then `#hide`
            // removal, then the recover-rank write scoped off the
            // post-removal line (so the hidden WRITE skip never applies
            // to a pulled-in R). Only a `[?]` R landing Ready can write
            // `[unblocked::]`; a Next landing skips it (UB4).
            let live = task.block_id.as_ref().is_some_and(|id| {
                post_live
                    .live
                    .contains_key(&(file.relative_path.clone(), id.clone()))
            });
            let landed = pull_in_recover_status(reason, task.status, live);
            let raw = raw_task_line(&file.contents, task.line_index);
            if landed == task.status && !pull_in_hide_removed(&raw) {
                continue;
            }
            let preview = pull_in_preview_line(&raw, landed);
            let scope = unblock_scope_for(&file.relative_path, &preview, live);
            let recovery = (task.status == '?' && landed == ' ')
                .then_some((input.today, scope));
            edit_line(
                &file.relative_path,
                task.line_index,
                Some(landed),
                None,
                recovery,
                false,
                true,
                None,
            );
            continue;
        }
        if task.status != '?' && verdict.offer.is_none() {
            continue;
        }
        let live = task.block_id.as_ref().is_some_and(|id| {
            post_live
                .live
                .contains_key(&(file.relative_path.clone(), id.clone()))
        });
        let landed = if task.status == '?' {
            if live {
                '*'
            } else {
                ' '
            }
        } else {
            task.status
        };
        if landed == task.status && verdict.offer.is_none() {
            continue;
        }
        let scope = unblock_scope_for(
            &file.relative_path,
            &raw_task_line(&file.contents, task.line_index),
            live,
        );
        let recovery = (landed == ' ').then_some((input.today, scope));
        edit_line(
            &file.relative_path,
            task.line_index,
            Some(landed),
            None,
            recovery,
            true,
            false,
            None,
        );
    }
    // PDC3: remaining open residents of a recovered project lose
    // `#prj_is_blocked` even when they keep their own status.
    let recovered_files =
        recovered_unblocked_project_files(&verdicts, &files, project_states);
    for file_index in recovered_files {
        let Some(state) = project_states.get(file_index) else {
            continue;
        };
        let context = &state.context;
        let file = &files[file_index];
        for (task_index, task) in file.tasks.iter().enumerate() {
            if !context.is_resident(task) {
                continue;
            }
            if verdicts.iter().any(|verdict| {
                verdict.file == file_index
                    && verdict.task == task_index
                    && matches!(verdict.kind, VerdictKind::Recover(_))
            }) {
                continue;
            }
            let current = raw_task_line(&file.contents, task.line_index);
            if normalize_prj_is_blocked_tag(&current, false) == current {
                continue;
            }
            edit_line(
                &file.relative_path,
                task.line_index,
                None,
                None,
                None,
                true,
                false,
                None,
            );
        }
    }
    // Compose one coherent day post-image when successor bullets were
    // inserted AND the day file itself carries task edits (a successor
    // living in the day file): re-run the same placements over the edited
    // day base. The layout is identical (status/mint edits are in-place),
    // so anchor coordinates stay valid. The day then rides only in
    // `new_day_text`, never in `changed_files`, so callers staging both
    // cannot overwrite one half with the other.
    let mut placed: Vec<PlacedSuccessor> = Vec::new();
    // Post-edit task locations in the day file by stable block identity,
    // including minted IDs: insertions shift lines, so snapshot line
    // numbers are stale for day rows.
    let mut day_task_lines: BTreeMap<String, usize> = BTreeMap::new();
    if !placements.is_empty() {
        if let Some(edited_day) = edited.remove(&input.day_relative) {
            let (coherent, coherent_placed) = insert_successor_links(
                &edited_day,
                &input.day_relative,
                input.index,
                placements,
            );
            for task in super::super::task_status_hooks::parse_tasks(
                &coherent,
                input.tasks_settings,
            ) {
                if let Some(id) = task.block_id.as_deref()
                    && !day_task_lines.contains_key(id)
                {
                    day_task_lines.insert(id.to_string(), task.line_index + 1);
                }
            }
            plan.new_day_text = Some(coherent);
            placed = coherent_placed;
        } else {
            let (post_day_text, post_placed) = insert_successor_links(
                &base_day_text,
                &input.day_relative,
                input.index,
                placements,
            );
            plan.new_day_text = Some(post_day_text);
            placed = post_placed;
        }
    }
    for (relative, contents) in &edited {
        let original = files
            .iter()
            .find(|file| &file.relative_path == relative)
            .map(|file| file.contents.as_str())
            .unwrap_or_default();
        if contents != original {
            plan.changed_files
                .insert(relative.clone(), contents.clone());
        }
    }
    // Rows report what the gesture did: linked successors always, recovers
    // whose status changed (pull-ins: or whose `#hide` was removed), and
    // still-blocked dependents always. `placed` aligns 1:1 with
    // `successor_positions` (insert returns one report per placement, in
    // input order). Row building lives in `project_pull_in` so this
    // planner stays small.
    emit_rows(
        &mut plan,
        RowEmit {
            files: &files,
            verdicts: &verdicts,
            successor_positions: &successor_positions,
            completed: &input.completed,
            surfaces,
            project_states,
            day: &day,
            closing_target: &closing_target,
            minted: &minted,
            placed,
            post_live: &post_live,
            day_task_lines: &day_task_lines,
            day_relative: &input.day_relative,
            index: input.index,
        },
    );
    plan
}

/// Insert successor bullets into the day text, reporting 1-based post
/// lines for the JSON `link` objects (§12.3, §12.5).
///
/// Slot anchors insert after the anchor bullet's subtree, in successor
/// order. Closing targets append after the target entry's existing
/// children (replacing a lone `\t- ` stub); a created placeholder goes
/// immediately after the closed entry's sub-bullet range. The inserted
/// line is always `<indent>- <link>` with nothing else on it, so it
/// counts as a Task Link.
pub(crate) fn insert_successor_links(
    day_text: &str,
    day_relative: &Path,
    index: &NoteIndex,
    placements: Vec<SuccessorPlacement>,
) -> (String, Vec<PlacedSuccessor>) {
    if placements.is_empty() {
        return (day_text.to_string(), Vec::new());
    }
    let day = DayView::parse(day_text, day_relative, index);
    let lines = logical_lines(day_text);
    let ending = if day_text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    struct Job {
        order: usize,
        insert_at: usize,
        entry: Option<usize>,
        created: bool,
        created_name: Option<String>,
        bullet: String,
    }
    let mut jobs = Vec::new();
    for (order, placement) in placements.into_iter().enumerate() {
        match placement.target {
            SuccessorTarget::Slot {
                entry_index,
                bullet_end,
                indent,
            } => jobs.push(Job {
                order,
                insert_at: bullet_end,
                entry: Some(entry_index),
                created: false,
                created_name: None,
                bullet: format!("{indent}- {}", placement.block_link),
            }),
            SuccessorTarget::Closing {
                entry_index,
                insert_at,
                created_name,
            } => {
                let created = entry_index.is_none() && created_name.is_some();
                jobs.push(Job {
                    order,
                    insert_at,
                    entry: entry_index,
                    created,
                    created_name,
                    bullet: format!("\t- {}", placement.block_link),
                });
            }
        }
    }
    // Group jobs by insertion point, preserving input order inside each
    // group (stable sort by position keeps successor order).
    jobs.sort_by_key(|job| job.insert_at);
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (position, job) in jobs.iter().enumerate() {
        let same = groups.last().and_then(|group| group.first()).is_some_and(
            |first| {
                let first_job = &jobs[*first];
                first_job.insert_at == job.insert_at
                    && first_job.entry == job.entry
                    && first_job.created == job.created
                    && first_job.created_name == job.created_name
            },
        );
        if same {
            groups.last_mut().expect("group").push(position);
        } else {
            groups.push(vec![position]);
        }
    }
    let mut segments: Vec<String> =
        lines.iter().map(|line| line.to_string()).collect();
    let had_trailing = day_text.ends_with('\n');
    // Net lines added at or below each position, for post coordinates.
    let mut added_below: BTreeMap<usize, usize> = BTreeMap::new();
    let mut bullet_post: BTreeMap<usize, usize> = BTreeMap::new();
    let mut created_post: BTreeMap<usize, usize> = BTreeMap::new();
    // (insert_at, net added) per applied group, ascending.
    let mut applied: Vec<(usize, usize)> = Vec::new();
    let shift_at = |applied: &[(usize, usize)], pre: usize| -> usize {
        applied
            .iter()
            .filter(|(position, _)| *position <= pre)
            .map(|(_, count)| count)
            .sum()
    };
    for group in &groups {
        let first = &jobs[group[0]];
        let at = first.insert_at.min(lines.len())
            + shift_at(&applied, first.insert_at);
        // Lone-stub replacement: a closing target into an existing entry
        // whose only child is a blank bullet takes that bullet's line for
        // the first successor instead of appending.
        let mut stub_at: Option<usize> = None;
        if !first.created
            && let Some(entry_index) = first.entry
            && let Some(entry) = day.entries.get(entry_index)
            && first.insert_at == entry.line + 2
            && let Some(stub) = lines.get(entry.line + 1)
            && stub.trim() == "-"
        {
            stub_at = Some(entry.line + 1 + shift_at(&applied, entry.line + 1));
        }
        let mut fresh: Vec<String> = Vec::new();
        if first.created {
            let name = first.created_name.clone().unwrap_or_default();
            fresh.push(if name.is_empty() {
                "- [ ] ()".to_string()
            } else {
                format!("- [ ] () — {name}")
            });
            let placeholder_post = at + fresh.len() - 1;
            for job_index in group {
                created_post.insert(jobs[*job_index].order, placeholder_post);
            }
        }
        for (position, job_index) in group.iter().enumerate() {
            let job = &jobs[*job_index];
            if position == 0
                && let Some(replaced) = stub_at
                && let Some(slot) = segments.get_mut(replaced)
            {
                *slot = job.bullet.clone();
                bullet_post.insert(job.order, replaced);
            } else {
                fresh.push(job.bullet.clone());
                bullet_post.insert(job.order, at + fresh.len() - 1);
            }
        }
        // The stub take is an in-place rewrite of an existing line, so
        // only spliced lines count as net additions (a lone stub taken by
        // a single successor adds `fresh.len()` lines, never -1).
        segments.splice(at..at, fresh.iter().cloned());
        let net = fresh.len();
        *added_below.entry(first.insert_at).or_default() += net;
        applied.push((first.insert_at, net));
    }
    let shift = |pre: usize| -> usize {
        added_below
            .iter()
            .filter(|(position, _)| **position <= pre)
            .map(|(_, count)| count)
            .sum()
    };
    let final_text = if segments.is_empty() {
        String::new()
    } else if had_trailing {
        segments.join(ending) + ending
    } else {
        segments.join(ending)
    };
    // `next_up`: the target entry is the first open untimed placeholder
    // in the resulting ledger — the one a bare `=` would start.
    let post_lines = logical_lines(&final_text);
    let post_section = pomodoros_section_range(&post_lines);
    let next_up_line = post_section
        .map(|section| scan_pomodoros(&post_lines, section))
        .and_then(|model| {
            model
                .entries
                .iter()
                .find(|entry| entry.open && !entry.timed)
                .map(|entry| entry.line_index)
        });
    let mut placed = BTreeMap::new();
    for job in &jobs {
        let line = bullet_post.get(&job.order).copied().unwrap_or(0) + 1;
        let entry_line = if job.created {
            created_post.get(&job.order).copied().unwrap_or(0) + 1
        } else if let Some(entry_index) = job.entry
            && let Some(entry) = day.entries.get(entry_index)
        {
            entry.line + shift(entry.line) + 1
        } else {
            0
        };
        placed.insert(
            job.order,
            PlacedSuccessor {
                line,
                entry_line,
                entry_created: job.created,
                next_up: next_up_line == Some(entry_line.saturating_sub(1)),
            },
        );
    }
    let mut ordered = Vec::new();
    for order in 0..jobs.len() {
        ordered.push(placed.remove(&order).unwrap_or(PlacedSuccessor {
            line: 0,
            entry_line: 0,
            entry_created: false,
            next_up: false,
        }));
    }
    (final_text, ordered)
}

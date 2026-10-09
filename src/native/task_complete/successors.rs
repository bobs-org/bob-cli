//! Pure successor-link planner (`docs/task-dependencies.md` §12).
//!
//! One definition, two engines: this is the Rust half of the shared rule.
//! Given the completed set C of one capture gesture, the staged post-close
//! snapshot, and the day text before the gesture, it decides per dependent
//! whether it stays blocked, recovers, or links into its predecessor's
//! slot. It returns note edits (status changes plus minted `^block-id`
//! appends, via [`set_task_line_status`]) and the ordered successor
//! placements for [`insert_successor_links`]. It never touches freshness,
//! schedules, or `[id::]`/`[dependsOn::]` fields, and never calls
//! `plan_task_link`.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use chrono::NaiveDate;

use super::super::{
    capture_block_ids::mint_block_id,
    capture_language::is_reserved_project_task_id,
    capture_pomodoros,
    capture_task_toggle::set_task_line_status,
    collect_done::block_ids_in_markdown,
    note_tasks::clean_description,
    note_tasks::NoteTaskSettings,
    pomodoro::pomodoros_section_range,
    task_dependencies::format::canonical_link,
    task_status_hooks::{
        logical_lines, scan_pomodoros, task_dependency_states, FileScan,
        TasksSettings,
    },
    vault_links::NoteIndex,
};
use crate::native::capture_dependency_tasks::has_hide_tag;

/// Successor-link breaker (`docs/task-dependencies.md` §12): more than
/// this many successors in one gesture links none.
pub(crate) const MAX_SUCCESSORS_PER_GESTURE: usize = 5;

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
    pub completed: Vec<CompletedTask>,
    pub snapshot: Vec<(PathBuf, String)>,
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
struct DayEntry {
    line: usize,
    name: String,
    open: bool,
}

/// Slot anchor: insert after an anchor bullet's subtree.
#[derive(Debug, Clone)]
struct SlotAnchor {
    entry_index: usize,
    bullet_end: usize,
    indent: String,
}

/// The pre-gesture day text parsed once: open entries plus the first
/// live link per completed identity, in ledger order.
struct DayView {
    entries: Vec<DayEntry>,
    live: BTreeMap<(PathBuf, String), (usize, usize, usize, String)>,
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
fn successor_link_text(
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
struct DependentVerdict {
    file: usize,
    task: usize,
    preds: Vec<usize>,
    anchor: Option<SlotAnchor>,
    kind: VerdictKind,
    open_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerdictKind {
    StillBlockedWaitsOn,
    StillBlockedScheduled,
    Recover(&'static str),
    Successor,
}

/// Run the shared successor rule (§12.2) over one gesture.
pub(crate) fn plan_successors(input: SuccessorInput) -> SuccessorPlan {
    // GATE: without an `[id::]` on any completed task no dependent can
    // name this close, so skip the lookup with no extra reads.
    if !input.completed.iter().any(|task| !task.task_id.is_empty()) {
        return SuccessorPlan::default();
    }
    let mut files = Vec::new();
    for (relative_path, contents) in &input.snapshot {
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
    let states = task_dependency_states(&files, &BTreeSet::new());
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
            if preds.is_empty() {
                continue;
            }
            let open_count = states
                .get(&(file_index, task_index))
                .map(|state| state.open_dependency_ids.len())
                .unwrap_or(0);
            let kind = if open_count > 0 {
                VerdictKind::StillBlockedWaitsOn
            } else if task.scheduled.is_some_and(|date| date > input.today) {
                VerdictKind::StillBlockedScheduled
            } else if task
                .block_id
                .as_deref()
                .is_some_and(is_reserved_project_task_id)
            {
                VerdictKind::Recover("project_task")
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
            } else {
                let anchor = earliest_anchor(&preds, &anchors);
                let linked = anchor.is_some() && input.link_unblocked;
                if anchor.is_none() {
                    VerdictKind::Recover("not_planned_today")
                } else if !linked {
                    VerdictKind::Recover("disabled")
                } else {
                    VerdictKind::Successor
                }
            };
            let anchor = match kind {
                VerdictKind::Successor => earliest_anchor(&preds, &anchors),
                _ => None,
            };
            verdicts.push(DependentVerdict {
                file: file_index,
                task: task_index,
                preds,
                anchor,
                kind,
                open_count,
            });
        }
    }

    // Order successors by anchor ledger position, then note path, then
    // line (§12.2 ORDER).
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

    // BREAKER: more than 5 successors in one gesture links none; each
    // recovers to its derived rank instead.
    if successor_positions.len() > MAX_SUCCESSORS_PER_GESTURE {
        for position in &successor_positions {
            verdicts[*position].kind = VerdictKind::Recover("breaker");
            verdicts[*position].anchor = None;
        }
        successor_positions.clear();
    }

    apply_verdicts(input, files, verdicts, successor_positions, day)
}

/// One resolved closing target (`docs/task-dependencies.md` §12.3):
/// where every successor of a Pomodoro close goes, plus the entry name
/// the JSON rows report and whether the entry is new since the gesture.
struct ResolvedClosingTarget {
    target: SuccessorTarget,
    name: String,
    entry_created: bool,
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
        .anchor
        .as_ref()
        .map(|anchor| (anchor.entry_index, anchor.bullet_end))
        .unwrap_or((usize::MAX, usize::MAX));
    let file = &files[verdict.file];
    let task = &file.tasks[verdict.task];
    (anchor_key, file.relative_path.clone(), task.line_index)
}

fn raw_task_line(contents: &str, line_index: usize) -> String {
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
) -> SuccessorPlan {
    let mut plan = SuccessorPlan {
        ran_lookup: true,
        ..SuccessorPlan::default()
    };
    // Mint successor block IDs in placement order so the per-note used
    // set (staged IDs plus IDs minted earlier in this gesture) is
    // deterministic.
    let mut minted: BTreeMap<(PathBuf, usize), String> = BTreeMap::new();
    let mut minted_in_note: BTreeMap<PathBuf, HashSet<String>> =
        BTreeMap::new();
    for position in &successor_positions {
        let verdict = &verdicts[*position];
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
        if let Some(resolved) = closing_target.as_ref() {
            placements.push(SuccessorPlacement {
                target: match &resolved.target {
                    SuccessorTarget::Closing {
                        entry_index,
                        insert_at,
                        created_name,
                    } => SuccessorTarget::Closing {
                        entry_index: *entry_index,
                        insert_at: *insert_at,
                        created_name: created_name.clone(),
                    },
                    SuccessorTarget::Slot { .. } => {
                        unreachable!("closing target is never a slot")
                    }
                },
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
    // ranks. Line layout never changes (in-place status swaps and
    // end-of-line appends), so snapshot line numbers stay valid.
    let mut edited: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut edit_line = |relative: &PathBuf,
                         line_index: usize,
                         new_status: Option<char>,
                         mint: Option<&str>| {
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
            if line.ends_with('\r') {
                updated.push('\r');
            }
            *line = updated;
        }
        edited.insert(relative.clone(), lines.join("\n"));
    };
    let successor_new_status = |previous: char| match previous {
        '?' | ' ' => '*',
        _ => previous,
    };
    for position in &successor_positions {
        let verdict = &verdicts[*position];
        let file = &files[verdict.file];
        let task = &file.tasks[verdict.task];
        let mint = minted
            .get(&(file.relative_path.clone(), task.line_index))
            .map(String::as_str);
        edit_line(
            &file.relative_path,
            task.line_index,
            Some(successor_new_status(task.status)),
            mint,
        );
    }
    // Recovered ranks derive from the post-gesture day text: `*` when the
    // dependent keeps a live link there, else ` `; only `?` moves.
    for verdict in verdicts.iter() {
        let VerdictKind::Recover(_) = verdict.kind else {
            continue;
        };
        let file = &files[verdict.file];
        let task = &file.tasks[verdict.task];
        if task.status != '?' {
            continue;
        }
        let live = task.block_id.as_ref().is_some_and(|id| {
            post_live
                .live
                .contains_key(&(file.relative_path.clone(), id.clone()))
        });
        edit_line(
            &file.relative_path,
            task.line_index,
            Some(if live { '*' } else { ' ' }),
            None,
        );
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
    let day_line = |note_path: &PathBuf, block_id: &str, fallback: usize| {
        if *note_path == input.day_relative && !block_id.is_empty() {
            day_task_lines.get(block_id).copied().unwrap_or(fallback)
        } else {
            fallback
        }
    };
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
    // Rows report what the gesture did: linked successors always, and
    // recovers whose status changed. Still-blocked dependents always.
    // `placed` aligns 1:1 with `successor_positions` (insert returns one
    // report per placement, in input order).
    let mut placed_by_order = placed.into_iter();
    for position in &successor_positions {
        let verdict = &verdicts[*position];
        let file = &files[verdict.file];
        let task = &file.tasks[verdict.task];
        let placed_link = placed_by_order.next().expect("placed link");
        let minted_id = minted
            .get(&(file.relative_path.clone(), task.line_index))
            .cloned();
        let block_id = task
            .block_id
            .clone()
            .or(minted_id.clone())
            .unwrap_or_default();
        let entry_name = match closing_target.as_ref() {
            Some(resolved) => resolved.name.clone(),
            None => {
                let anchor =
                    verdict.anchor.as_ref().expect("successor has an anchor");
                day.entries
                    .get(anchor.entry_index)
                    .map(|entry| entry.name.clone())
                    .unwrap_or_default()
            }
        };
        let block_link = successor_link_text(
            &file.relative_path,
            &block_id,
            &input.day_relative,
            input.index,
        );
        let line =
            day_line(&file.relative_path, &block_id, task.line_index + 1);
        plan.unblocked.push(UnblockedRow {
            note_path: file.relative_path.clone(),
            block_id,
            line,
            text: task.description.clone(),
            previous_status_symbol: task.status,
            status_symbol: successor_new_status(task.status),
            inbox: is_inbox_note(
                &file.relative_path,
                &file.contents,
                input.index,
            ),
            unblocked_by: input
                .completed
                .iter()
                .enumerate()
                .filter(|(position, _)| verdict.preds.contains(position))
                .map(|(_, completed)| PredecessorRef {
                    note_path: completed.relative_path.clone(),
                    block_id: completed.block_id.clone(),
                    text: completed.text.clone(),
                })
                .collect(),
            link: Some(SuccessorLinkReport {
                entry_name,
                entry_line: placed_link.entry_line,
                line: placed_link.line,
                block_link,
                entry_created: placed_link.entry_created
                    || closing_target
                        .as_ref()
                        .is_some_and(|resolved| resolved.entry_created),
                next_up: placed_link.next_up,
                block_id_created: minted_id.is_some(),
            }),
            not_linked: None,
        });
    }
    for verdict in verdicts.iter() {
        let file = &files[verdict.file];
        let task = &file.tasks[verdict.task];
        match verdict.kind {
            VerdictKind::StillBlockedWaitsOn => {
                let block_id = task.block_id.clone().unwrap_or_default();
                let line = day_line(
                    &file.relative_path,
                    &block_id,
                    task.line_index + 1,
                );
                plan.still_blocked.push(StillBlockedRow {
                    note_path: file.relative_path.clone(),
                    block_id,
                    line,
                    text: task.description.clone(),
                    status_symbol: task.status,
                    reason: "waits_on",
                    waits_on: verdict.open_count,
                    scheduled: None,
                });
            }
            VerdictKind::StillBlockedScheduled => {
                let block_id = task.block_id.clone().unwrap_or_default();
                let line = day_line(
                    &file.relative_path,
                    &block_id,
                    task.line_index + 1,
                );
                plan.still_blocked.push(StillBlockedRow {
                    note_path: file.relative_path.clone(),
                    block_id,
                    line,
                    text: task.description.clone(),
                    status_symbol: task.status,
                    reason: "scheduled",
                    waits_on: 0,
                    scheduled: task
                        .scheduled
                        .map(|date| date.format("%Y-%m-%d").to_string()),
                });
            }
            VerdictKind::Recover(reason) => {
                if task.status != '?' {
                    continue;
                }
                let live = task.block_id.as_ref().is_some_and(|id| {
                    post_live
                        .live
                        .contains_key(&(file.relative_path.clone(), id.clone()))
                });
                let status_symbol = if live { '*' } else { ' ' };
                if status_symbol == task.status {
                    continue;
                }
                let block_id = task.block_id.clone().unwrap_or_default();
                let line = day_line(
                    &file.relative_path,
                    &block_id,
                    task.line_index + 1,
                );
                plan.unblocked.push(UnblockedRow {
                    note_path: file.relative_path.clone(),
                    block_id,
                    line,
                    text: task.description.clone(),
                    previous_status_symbol: task.status,
                    status_symbol,
                    inbox: is_inbox_note(
                        &file.relative_path,
                        &file.contents,
                        input.index,
                    ),
                    unblocked_by: input
                        .completed
                        .iter()
                        .enumerate()
                        .filter(|(position, _)| {
                            verdict.preds.contains(position)
                        })
                        .map(|(_, completed)| PredecessorRef {
                            note_path: completed.relative_path.clone(),
                            block_id: completed.block_id.clone(),
                            text: completed.text.clone(),
                        })
                        .collect(),
                    link: None,
                    not_linked: Some(reason),
                });
            }
            VerdictKind::Successor => {}
        }
    }
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

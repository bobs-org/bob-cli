//! Batch-level parent-task block tracking for `bob capture`.
//!
//! Every sub-bullet item reports its parent through a [`TaskBlockRef`].
//! [`plan_capture_batch`](super::plan_capture_batch) owns a
//! [`TaskBlockTracker`] that forwards parent positions across items and
//! reports each distinct parent once, in first-touch order, in its final
//! state, with the cumulative diff against the note before the capture.
use super::*;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

/// Informational role a capture played for one task block, in
/// first-touch order. The app does not depend on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum TaskBlockRole {
    SubBullet,
    Dependency,
    DependencyTarget,
    Completed,
    Unblocked,
}

/// One sub-bullet touch of a parent task. `line` is the 0-based parent
/// line in that item's post-state text. This type is never serialized;
/// [`PlannedCaptureItem`](super::PlannedCaptureItem) carries it only so
/// the batch loop can feed the tracker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TaskBlockRef {
    pub(super) target: PathBuf,
    pub(super) relative_target: String,
    pub(super) route: String,
    pub(super) line: usize,
    pub(super) block_id: Option<String>,
    pub(super) role: TaskBlockRole,
}

/// One parent tracked across the batch: its current 0-based line in the
/// latest post-state text, plus the identity needed to re-find it after
/// rewrites and to render the final block.
struct TrackedTaskBlock {
    target: PathBuf,
    relative_target: String,
    route: String,
    current: usize,
    block_id: Option<String>,
    roles: Vec<TaskBlockRole>,
}

/// Batch-scoped tracker for parent-task blocks across route notes. Feed
/// every item in order (with a pre-item snapshot of each tracked target),
/// then call [`finish`](Self::finish) with the planner.
pub(super) struct TaskBlockTracker {
    tracked: Vec<TrackedTaskBlock>,
    settings: note_tasks::NoteTaskSettings,
}

impl TaskBlockTracker {
    pub(super) fn new(settings: note_tasks::NoteTaskSettings) -> Self {
        Self {
            tracked: Vec::new(),
            settings,
        }
    }

    /// Targets currently tracked, for the caller to snapshot before
    /// planning the next item.
    fn tracked_targets(&self) -> Vec<PathBuf> {
        let mut seen = std::collections::BTreeSet::new();
        let mut targets = Vec::new();
        for tracked in &self.tracked {
            if seen.insert(tracked.target.clone()) {
                targets.push(tracked.target.clone());
            }
        }
        targets
    }

    /// Snapshot the staged text of every tracked target. Call before
    /// planning each item; pass the map to
    /// [`track_item`](Self::track_item) afterwards.
    pub(super) fn snapshot_pre(
        &self,
        planner: &CaptureBatchPlanner,
    ) -> HashMap<PathBuf, Option<String>> {
        self.tracked_targets()
            .into_iter()
            .map(|target| {
                let text = planner.peek_text(&target);
                (target, text)
            })
            .collect()
    }

    /// Forward tracked parents whose note changed, then merge this item's
    /// refs. Only notes the item actually changed pay for a Myers map.
    pub(super) fn track_item(
        &mut self,
        planner: &CaptureBatchPlanner,
        pre: &HashMap<PathBuf, Option<String>>,
        refs: Vec<TaskBlockRef>,
    ) {
        // Group tracked indices by target so each changed note maps once.
        let mut by_target: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
        for (index, tracked) in self.tracked.iter().enumerate() {
            by_target
                .entry(tracked.target.clone())
                .or_default()
                .push(index);
        }
        for (target, indices) in by_target {
            let pre_text = pre.get(&target).and_then(|text| text.clone());
            let post_text = planner.peek_text(&target);
            let (Some(pre_text), Some(post_text)) = (pre_text, post_text)
            else {
                // A tracked note should stay loaded; a missing side means
                // a planner bug. Keep positions rather than guess.
                debug_assert!(
                    false,
                    "tracked task note missing before/after text"
                );
                continue;
            };
            if pre_text == post_text {
                continue;
            }
            let pre_lines: Vec<&str> = pre_text.lines().collect();
            let post_lines: Vec<&str> = post_text.lines().collect();
            let (old_to_new, _) = line_maps(&pre_lines, &post_lines);
            let post_scan = note_tasks::scan(&post_text, &self.settings);
            let mut drop_block = Vec::new();
            for index in indices {
                let tracked = &mut self.tracked[index];
                if let Some(mapped) = old_to_new.get(&tracked.current) {
                    tracked.current = *mapped;
                    continue;
                }
                // The task line was rewritten: re-find by block ID, then
                // by identical bytes, else drop.
                if let Some(found) =
                    tracked.block_id.as_deref().and_then(|id| {
                        match post_scan.by_block_id(id) {
                            BlockIdLookup::Found(task) => Some(task.line_index),
                            _ => None,
                        }
                    })
                {
                    tracked.current = found;
                } else if let Some(moved) = find_moved_task_line(
                    pre_lines.get(tracked.current).copied(),
                    &post_lines,
                    &post_scan,
                    tracked.current,
                ) {
                    tracked.current = moved;
                } else {
                    debug_assert!(
                        false,
                        "tracked task headline lost without a ref"
                    );
                    drop_block.push(index);
                }
            }
            // Remove dropped blocks (indices descending to keep positions).
            for index in drop_block.into_iter().rev() {
                self.tracked.remove(index);
            }
        }
        self.merge_or_start(refs);
    }

    /// Fold refs into tracked blocks by post-state target+line, or start
    /// a new tracked parent.
    fn merge_or_start(&mut self, refs: Vec<TaskBlockRef>) {
        for item in refs {
            if let Some(tracked) = self.tracked.iter_mut().find(|entry| {
                entry.target == item.target && entry.current == item.line
            }) {
                if !tracked.roles.contains(&item.role) {
                    tracked.roles.push(item.role);
                }
                continue;
            }
            self.tracked.push(TrackedTaskBlock {
                target: item.target,
                relative_target: item.relative_target,
                route: item.route,
                current: item.line,
                block_id: item.block_id,
                roles: vec![item.role],
            });
        }
    }

    /// Re-extract every tracked parent from its final staged text and diff
    /// it against its pre-batch bytes. One scan per tracked note; Myers
    /// runs once per note pair.
    pub(super) fn finish(
        self,
        planner: &CaptureBatchPlanner,
    ) -> Vec<TaskBlockJson> {
        let settings = self.settings.clone();
        let tracked_all = self.tracked;
        // Group tracked blocks by target so each note scans once, carrying
        // the global first-touch ordinal so output restores batch order
        // rather than path order.
        let mut by_target: BTreeMap<PathBuf, Vec<(usize, TrackedTaskBlock)>> =
            BTreeMap::new();
        for (ordinal, tracked) in tracked_all.into_iter().enumerate() {
            by_target
                .entry(tracked.target.clone())
                .or_default()
                .push((ordinal, tracked));
        }
        let mut ordered: Vec<(usize, TaskBlockJson)> = Vec::new();
        for (target, group) in by_target {
            // `group` preserves first-touch order within the note because it
            // was built by iterating `tracked` in order.
            let Some((original, final_text)) = planner.loaded_texts(&target)
            else {
                debug_assert!(false, "tracked task note was never loaded");
                continue;
            };
            let orig_lines: Vec<&str> = original.lines().collect();
            let final_lines: Vec<&str> = final_text.lines().collect();
            let (_, new_to_old) = line_maps(&orig_lines, &final_lines);
            let orig_scan = note_tasks::scan(&original, &settings);
            let final_scan = note_tasks::scan(&final_text, &settings);
            for (ordinal, tracked) in group {
                let Some(final_task) =
                    final_scan.task_at(tracked.current).or_else(|| {
                        // The forwarded line should be a task; fall back to
                        // a block-ID lookup before giving up.
                        tracked.block_id.as_deref().and_then(|id| {
                            match final_scan.by_block_id(id) {
                                BlockIdLookup::Found(task) => Some(task),
                                _ => None,
                            }
                        })
                    })
                else {
                    debug_assert!(false, "tracked task headline is not a task");
                    continue;
                };
                // Use the resolved final task's line (usually the tracked
                // line) so a block-ID fallback still reports the right
                // block.
                let final_line = final_task.line_index;
                let Some(final_range) =
                    task_block_line_range(&final_text, final_task)
                else {
                    debug_assert!(false, "task block range missing");
                    continue;
                };
                let final_block = final_range
                    .clone()
                    .map(|index| {
                        final_lines
                            .get(index)
                            .copied()
                            .unwrap_or("")
                            .to_string()
                    })
                    .collect::<Vec<_>>();
                let final_depths =
                    block_depths(&final_lines, final_line, final_range);
                // Map the final task line back to the original.
                let (before_block, created) = match new_to_old.get(&final_line)
                {
                    Some(mapped) => {
                        match orig_scan.task_at(*mapped) {
                            Some(orig_task) => {
                                match task_block_line_range(
                                    &original, orig_task,
                                ) {
                                    Some(range) => {
                                        let before = range
                                            .map(|index| {
                                                orig_lines
                                                    .get(index)
                                                    .copied()
                                                    .unwrap_or("")
                                                    .to_string()
                                            })
                                            .collect::<Vec<_>>();
                                        (before, false)
                                    }
                                    None => {
                                        debug_assert!(
                                            false,
                                            "original task block range missing"
                                        );
                                        continue;
                                    }
                                }
                            }
                            None => {
                                // Mapped to a non-task line: try the
                                // block-ID / identical fallbacks below.
                                match resolve_unmapped_before(
                                    &original,
                                    &orig_lines,
                                    &orig_scan,
                                    &final_lines,
                                    final_line,
                                    tracked.block_id.as_deref(),
                                ) {
                                    Some(result) => result,
                                    None => {
                                        debug_assert!(
                                            false,
                                            "mapped task line is not a task"
                                        );
                                        continue;
                                    }
                                }
                            }
                        }
                    }
                    None => {
                        match resolve_unmapped_before(
                            &original,
                            &orig_lines,
                            &orig_scan,
                            &final_lines,
                            final_line,
                            tracked.block_id.as_deref(),
                        ) {
                            Some(result) => result,
                            None => continue,
                        }
                    }
                };
                let before_refs =
                    before_block.iter().map(String::as_str).collect::<Vec<_>>();
                let before_depths = if created || before_block.is_empty() {
                    Vec::new()
                } else {
                    // The before-block starts at its task line, so the
                    // headline index is always 0 in the sliced block.
                    block_depths(&before_refs, 0, 0..before_block.len())
                };
                let lines = pair_lines(
                    &before_block,
                    &before_depths,
                    &final_block,
                    &final_depths,
                    created,
                );
                ordered.push((
                    ordinal,
                    TaskBlockJson {
                        relative_target: tracked.relative_target,
                        route: tracked.route,
                        line: final_line + 1,
                        block_id: final_task.block_id.clone(),
                        text: final_task.description.clone(),
                        status_symbol: final_task.status_symbol,
                        status_name: final_task.status_name.clone(),
                        created,
                        roles: tracked.roles,
                        lines,
                    },
                ));
            }
        }
        ordered.sort_by_key(|(ordinal, _)| *ordinal);
        ordered.into_iter().map(|(_, block)| block).collect()
    }
}

/// Resolve the before-block for a final task line with no whole-note
/// mapping: a missing block ID means the parent was created by this
/// batch; otherwise prefer the block-ID task, falling back to the
/// nearest identical task line. `None` means skip the block.
fn resolve_unmapped_before(
    original: &str,
    orig_lines: &[&str],
    orig_scan: &note_tasks::NoteTaskScan,
    final_lines: &[&str],
    final_line: usize,
    block_id: Option<&str>,
) -> Option<(Vec<String>, bool)> {
    if let Some(id) = block_id {
        match orig_scan.by_block_id(id) {
            BlockIdLookup::Missing => return Some((Vec::new(), true)),
            BlockIdLookup::Found(task) => {
                let Some(range) = task_block_line_range(original, task) else {
                    debug_assert!(false, "original task block range missing");
                    return None;
                };
                let before = range
                    .map(|index| {
                        orig_lines.get(index).copied().unwrap_or("").to_string()
                    })
                    .collect::<Vec<_>>();
                return Some((before, false));
            }
            BlockIdLookup::Duplicate(_) | BlockIdLookup::NotATask { .. } => {
                // Fall through to the identical-bytes search.
            }
        }
    }
    let wanted = final_lines.get(final_line).copied();
    let Some(found) =
        find_moved_task_line(wanted, orig_lines, orig_scan, final_line)
    else {
        // An ID-less line with no pre-image is a task this batch
        // created (dependency captures report new dependents without a
        // user-authored ID): empty before, created after.
        if block_id.is_none() {
            return Some((Vec::new(), true));
        }
        debug_assert!(false, "task block has no before match");
        return None;
    };
    let Some(orig_task) = orig_scan.task_at(found) else {
        debug_assert!(false, "identical task line is not a task");
        return None;
    };
    let Some(range) = task_block_line_range(original, orig_task) else {
        debug_assert!(false, "original task block range missing");
        return None;
    };
    let before = range
        .map(|index| orig_lines.get(index).copied().unwrap_or("").to_string())
        .collect::<Vec<_>>();
    Some((before, false))
}

/// Exact-text fallback for a task line Myers left unmapped: search the
/// other snapshot's task lines for the same bytes. Nearest to the hint
/// wins; `None` when no task carries that text.
fn find_moved_task_line(
    wanted: Option<&str>,
    other_lines: &[&str],
    other_scan: &note_tasks::NoteTaskScan,
    hint: usize,
) -> Option<usize> {
    let wanted = wanted?;
    other_scan
        .tasks()
        .iter()
        .map(|task| task.line_index)
        .filter(|index| other_lines.get(*index).copied() == Some(wanted))
        .min_by_key(|index| index.abs_diff(hint))
}

/// Line range (0-based, end-exclusive) of a task's block in `contents`,
/// from the task line through its `block_end` byte offset.
fn task_block_line_range(
    contents: &str,
    task: &note_tasks::NoteTask,
) -> Option<std::ops::Range<usize>> {
    let mut end = 0usize;
    let mut line_ends = Vec::new();
    for segment in contents.split_inclusive('\n') {
        end += segment.len();
        line_ends.push(end);
    }
    // A file without a trailing newline leaves a final segment without
    // `\n` in the loop above only when it contains `\n`; handle the
    // trailing unterminated line the same way `note_lines` does.
    if contents.is_empty() {
        return None;
    }
    if !contents.ends_with('\n') && !line_ends.iter().any(|_| false) {
        // split_inclusive already yielded the trailing segment, so the
        // ends above are complete; this branch only documents parity.
    }
    let last = line_ends
        .iter()
        .position(|line_end| *line_end == task.block_end)?;
    if last < task.line_index {
        return None;
    }
    Some(task.line_index..last + 1)
}

/// One parent task in its final state, at batch level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TaskBlockJson {
    pub(super) relative_target: String,
    pub(super) route: String,
    pub(super) line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) block_id: Option<String>,
    pub(super) text: String,
    pub(super) status_symbol: char,
    pub(super) status_name: String,
    pub(super) created: bool,
    pub(super) roles: Vec<TaskBlockRole>,
    pub(super) lines: Vec<BlockLineJson>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS_DATA_JSON: &str = r##"{
          "globalFilter": "#task",
          "statusSettings": {
            "coreStatuses": [
              {"symbol":" ","name":"Todo","type":"TODO"},
              {"symbol":"x","name":"Done","type":"DONE"},
              {"symbol":"/","name":"In Progress","type":"IN_PROGRESS"},
              {"symbol":"*","name":"Next","type":"ON_HOLD"},
              {"symbol":"-","name":"Canceled","type":"CANCELLED"}
            ],
            "customStatuses": []
          }
        }"##;

    fn write_vault(dir: &tempfile::TempDir, files: &[(&str, &str)]) -> PathBuf {
        let vault = dir.path().to_path_buf();
        std::fs::create_dir_all(
            vault.join(".obsidian/plugins/obsidian-tasks-plugin"),
        )
        .expect("create tasks plugin dir");
        std::fs::write(
            vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
            STATUS_DATA_JSON,
        )
        .expect("write data.json");
        for (name, contents) in files {
            if let Some(parent) = Path::new(name).parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(vault.join(parent))
                    .expect("create parent");
            }
            std::fs::write(vault.join(name), contents).expect("write note");
        }
        vault
    }

    fn dry_request(
        vault: &Path,
        raw_text: &str,
        forced_route: Option<String>,
        forced_target: Option<SubBulletTarget>,
    ) -> CaptureRequest {
        CaptureRequest {
            bob_dir: vault.to_path_buf(),
            dry_run: true,
            forced_clip: None,
            forced_destination_flags: Vec::new(),
            forced_route,
            forced_section: None,
            forced_sub_bullet_target: forced_target,
            forced_task_section: None,
            no_clip: true,
            raw_text: raw_text.to_string(),
        }
    }

    fn plan(vault: &Path, raw_text: &str) -> PlannedCaptureBatch {
        plan_capture_batch(&dry_request(vault, raw_text, None, None))
            .expect("plan capture batch")
    }

    fn row_tuples(
        block: &TaskBlockJson,
    ) -> Vec<(&str, usize, BlockLineChange)> {
        block
            .lines
            .iter()
            .map(|row| (row.text.as_str(), row.depth, row.change))
            .collect()
    }

    #[test]
    fn block_range_maps_byte_end_to_line_range() {
        let contents = "- [ ] #task A ^a\n\t- child\n\t- 🗓️ **SCHEDULE LOG**\n";
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = write_vault(&dir, &[("sase.md", contents)]);
        let scan =
            note_tasks::scan(contents, &note_tasks::read_settings(&vault));
        let task = scan.task_at(0).expect("task");
        let range = task_block_line_range(contents, task).expect("range");
        assert_eq!(range, 0..3);
    }

    #[test]
    fn plain_insertion_reports_one_added_row_before_schedule_log() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = write_vault(
            &dir,
            &[(
                "sase.md",
                concat!(
                    "- [/] #task Port capture to PIW sase-core [created:: 2026-09-30] ^capture\n",
                    "\t- REQUIREMENTS\n",
                    "\t\t- existing\n",
                    "\t- 🗓️ **SCHEDULE LOG**\n",
                    "\t\t- 2026-10-01 moved\n",
                ),
            )],
        );
        let batch = plan(
            &vault,
            "Should reuse as much of PIW sase-core code as possible! @sase+capture",
        );
        assert_eq!(batch.task_blocks.len(), 1);
        let block = &batch.task_blocks[0];
        assert_eq!(block.relative_target, "sase.md");
        assert_eq!(block.route, "sase");
        assert_eq!(block.line, 1);
        assert_eq!(block.block_id.as_deref(), Some("capture"));
        assert_eq!(block.text, "Port capture to PIW sase-core");
        assert_eq!(block.status_symbol, '/');
        assert_eq!(block.status_name, "In Progress");
        assert!(!block.created);
        assert_eq!(block.roles, vec![TaskBlockRole::SubBullet]);
        assert_eq!(
            row_tuples(block),
            vec![
                (
                    "- [/] #task Port capture to PIW sase-core [created:: 2026-09-30] ^capture",
                    0,
                    BlockLineChange::Unchanged
                ),
                ("\t- REQUIREMENTS", 1, BlockLineChange::Unchanged),
                ("\t\t- existing", 2, BlockLineChange::Unchanged),
                (
                    "\t- Should reuse as much of PIW sase-core code as possible!",
                    1,
                    BlockLineChange::Added
                ),
                ("\t- 🗓️ **SCHEDULE LOG**", 1, BlockLineChange::Unchanged),
                ("\t\t- 2026-10-01 moved", 2, BlockLineChange::Unchanged),
            ]
        );
        assert!(block.lines.iter().all(|row| row.before.is_none()));
    }

    #[test]
    fn section_insertion_nests_the_added_row_at_depth_two() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = write_vault(
            &dir,
            &[(
                "foo.md",
                concat!(
                    "- [ ] #task Upgrade Postgres [created::2026-07-31] ^bar\n",
                    "\t- REQUIREMENTS\n",
                    "\t\t- existing\n",
                    "\t- FUTURE WORK\n",
                ),
            )],
        );
        let batch = plan(&vault, "Postgres 17 minimum @foo+bar#requirements");
        assert_eq!(batch.task_blocks.len(), 1);
        let block = &batch.task_blocks[0];
        assert_eq!(block.block_id.as_deref(), Some("bar"));
        assert_eq!(
            row_tuples(block),
            vec![
                (
                    "- [ ] #task Upgrade Postgres [created::2026-07-31] ^bar",
                    0,
                    BlockLineChange::Unchanged
                ),
                ("\t- REQUIREMENTS", 1, BlockLineChange::Unchanged),
                ("\t\t- existing", 2, BlockLineChange::Unchanged),
                ("\t\t- Postgres 17 minimum", 2, BlockLineChange::Added),
                ("\t- FUTURE WORK", 1, BlockLineChange::Unchanged),
            ]
        );
    }

    #[test]
    fn authored_children_and_schedule_log_are_added_in_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = write_vault(
            &dir,
            &[(
                "sase.md",
                concat!("- [/] #task Parent ^capture\n", "\t- keep\n",),
            )],
        );
        let batch = plan(&vault, "new note @sase+capture p:1\n- child one");
        assert_eq!(batch.task_blocks.len(), 1);
        let block = &batch.task_blocks[0];
        assert_eq!(block.lines[0].change, BlockLineChange::Unchanged);
        assert_eq!(block.lines[1].change, BlockLineChange::Unchanged);
        let added = &block.lines[2..];
        assert!(
            added.iter().all(|row| row.change == BlockLineChange::Added),
            "new bullet, children, and schedule log are all added: {added:?}"
        );
        assert_eq!(block.lines[2].depth, 1);
        assert!(block.lines[2].text.contains("new note"));
        assert_eq!(block.lines[3].depth, 2);
        assert!(block.lines[3].text.contains("child one"));
        let schedule_at = block
            .lines
            .iter()
            .position(|row| row.text.contains("SCHEDULE LOG"))
            .expect("generated schedule log");
        assert_eq!(block.lines[schedule_at].depth, 2);
        assert_eq!(block.lines[schedule_at + 1].depth, 3);
    }

    #[test]
    fn global_batch_reports_one_block_with_two_added_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = write_vault(
            &dir,
            &[(
                "sase.md",
                concat!("- [/] #task Parent ^capture\n", "\t- keep\n",),
            )],
        );
        let batch = plan(&vault, "@@sase+capture\nfirst note\n\nsecond note");
        assert_eq!(batch.task_blocks.len(), 1);
        let block = &batch.task_blocks[0];
        assert_eq!(block.block_id.as_deref(), Some("capture"));
        assert_eq!(block.roles, vec![TaskBlockRole::SubBullet]);
        assert_eq!(
            row_tuples(block),
            vec![
                ("- [/] #task Parent ^capture", 0, BlockLineChange::Unchanged),
                ("\t- keep", 1, BlockLineChange::Unchanged),
                ("\t- first note", 1, BlockLineChange::Added),
                ("\t- second note", 1, BlockLineChange::Added),
            ]
        );
    }

    #[test]
    fn two_parents_forward_across_an_insert_above() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = write_vault(
            &dir,
            &[(
                "sase.md",
                concat!(
                    "- [ ] #task First ^aaa\n",
                    "\t- keep a\n",
                    "- [ ] #task Second ^bbb\n",
                    "\t- keep b\n",
                ),
            )],
        );
        let batch = plan(
            &vault,
            "note under bbb @sase+bbb\n\nnote under aaa @sase+aaa",
        );
        assert_eq!(batch.task_blocks.len(), 2);
        // First-touch order: bbb, then aaa.
        assert_eq!(batch.task_blocks[0].block_id.as_deref(), Some("bbb"));
        assert_eq!(batch.task_blocks[1].block_id.as_deref(), Some("aaa"));
        // The second item inserted above bbb, so bbb sits at line 4 final.
        assert_eq!(batch.task_blocks[0].line, 4);
        assert_eq!(batch.task_blocks[1].line, 1);
        assert_eq!(
            row_tuples(&batch.task_blocks[0]),
            vec![
                ("- [ ] #task Second ^bbb", 0, BlockLineChange::Unchanged),
                ("\t- keep b", 1, BlockLineChange::Unchanged),
                ("\t- note under bbb", 1, BlockLineChange::Added),
            ]
        );
        assert_eq!(
            row_tuples(&batch.task_blocks[1]),
            vec![
                ("- [ ] #task First ^aaa", 0, BlockLineChange::Unchanged),
                ("\t- keep a", 1, BlockLineChange::Unchanged),
                ("\t- note under aaa", 1, BlockLineChange::Added),
            ]
        );
    }

    #[test]
    fn created_parent_reports_every_row_added() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = write_vault(&dir, &[("sase.md", "")]);
        let batch =
            plan(&vault, "My task @sase^new\n\nnote under new @sase+new");
        assert_eq!(batch.task_blocks.len(), 1);
        let block = &batch.task_blocks[0];
        assert_eq!(block.block_id.as_deref(), Some("new"));
        assert!(block.created);
        assert_eq!(block.lines.len(), 2);
        assert!(
            block
                .lines
                .iter()
                .all(|row| row.change == BlockLineChange::Added
                    && row.before.is_none()),
            "created blocks are all added: {:?}",
            block.lines
        );
        assert_eq!(
            block.lines.iter().map(|row| row.depth).collect::<Vec<_>>(),
            vec![0, 1]
        );
    }

    #[test]
    fn toggle_plus_subbullet_shows_the_task_line_as_changed() {
        // Direct tracker coverage for a batch whose first item rewrites
        // the parent's task line and whose second item adds a sub-bullet:
        // the cumulative diff reports the task line as changed.
        let dir = tempfile::tempdir().expect("tempdir");
        let target = PathBuf::from("sase.md");
        let absolute = dir.path().join(&target);
        let original = "- [ ] #task Parent ^capture\n\t- keep\n";
        let toggled = "- [*] #task Parent ^capture\n\t- keep\n";
        let both = "- [*] #task Parent ^capture\n\t- keep\n\t- note\n";
        std::fs::create_dir_all(dir.path()).expect("create dir");
        std::fs::write(&absolute, original).expect("write original");
        let mut planner = CaptureBatchPlanner::default();
        planner.ensure_loaded(&absolute).expect("load");
        let settings = note_tasks::read_settings(dir.path());
        let mut tracker = TaskBlockTracker::new(settings);
        // Before the toggle, nothing is tracked.
        let pre = tracker.snapshot_pre(&planner);
        planner
            .stage(&absolute, toggled.to_string())
            .expect("stage toggle");
        tracker.track_item(&planner, &pre, Vec::new());
        // The sub-bullet item reports its parent ref.
        let pre = tracker.snapshot_pre(&planner);
        planner
            .stage(&absolute, both.to_string())
            .expect("stage bullet");
        tracker.track_item(
            &planner,
            &pre,
            vec![TaskBlockRef {
                target: absolute.clone(),
                relative_target: "sase.md".to_string(),
                route: "sase".to_string(),
                line: 0,
                block_id: Some("capture".to_string()),
                role: TaskBlockRole::SubBullet,
            }],
        );
        let blocks = tracker.finish(&planner);
        assert_eq!(blocks.len(), 1);
        let block = &blocks[0];
        assert!(!block.created);
        assert_eq!(block.lines[0].change, BlockLineChange::Changed);
        assert_eq!(
            block.lines[0].before.as_deref(),
            Some("- [ ] #task Parent ^capture")
        );
        assert_eq!(block.lines[0].text, "- [*] #task Parent ^capture");
        assert_eq!(block.lines[1].change, BlockLineChange::Unchanged);
        assert_eq!(block.lines[2].change, BlockLineChange::Added);
        let _ = target;
    }

    #[test]
    fn task_ref_parent_omits_block_id_but_reports_its_block() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = write_vault(
            &dir,
            &[("sase.md", "- [ ] #task No ID parent\n\t- keep\n")],
        );
        let scan = note_tasks::scan(
            "- [ ] #task No ID parent\n\t- keep\n",
            &note_tasks::read_settings(&vault),
        );
        let task = scan.task_at(0).expect("task");
        assert!(task.block_id.is_none());
        let task_ref = note_tasks::TaskRef::from_task(task);
        let request = dry_request(
            &vault,
            "hello",
            Some("sase".to_string()),
            Some(SubBulletTarget::Ref {
                line: task_ref.line,
                digest: task_ref.digest,
            }),
        );
        let batch = plan_capture_batch(&request).expect("plan");
        assert_eq!(batch.task_blocks.len(), 1);
        let block = &batch.task_blocks[0];
        assert!(block.block_id.is_none());
        assert_eq!(block.text, "No ID parent");
        assert_eq!(
            row_tuples(block),
            vec![
                ("- [ ] #task No ID parent", 0, BlockLineChange::Unchanged),
                ("\t- keep", 1, BlockLineChange::Unchanged),
                ("\t- hello", 1, BlockLineChange::Added),
            ]
        );
        let value = serde_json::to_value(block).expect("serialize task block");
        assert!(value.get("block_id").is_none(), "{value}");
    }

    #[test]
    fn nested_parent_reports_depths_relative_to_its_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = write_vault(
            &dir,
            &[(
                "sase.md",
                concat!(
                    "- [ ] #task Root\n",
                    "  - [ ] #task Nested ^parent\n",
                    "    - existing\n",
                ),
            )],
        );
        let batch = plan(&vault, "new note @sase+parent");
        assert_eq!(batch.task_blocks.len(), 1);
        let block = &batch.task_blocks[0];
        assert_eq!(
            row_tuples(block),
            vec![
                (
                    "  - [ ] #task Nested ^parent",
                    0,
                    BlockLineChange::Unchanged
                ),
                ("    - existing", 1, BlockLineChange::Unchanged),
                ("    - new note", 1, BlockLineChange::Added),
            ]
        );
    }

    #[test]
    fn crlf_note_reports_verbatim_texts_without_terminators() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = write_vault(
            &dir,
            &[("sase.md", "- [ ] #task Parent ^capture\r\n\t- keep\r\n")],
        );
        let batch = plan(&vault, "note @sase+capture");
        assert_eq!(batch.task_blocks.len(), 1);
        let block = &batch.task_blocks[0];
        assert_eq!(
            row_tuples(block),
            vec![
                ("- [ ] #task Parent ^capture", 0, BlockLineChange::Unchanged),
                ("\t- keep", 1, BlockLineChange::Unchanged),
                ("\t- note", 1, BlockLineChange::Added),
            ]
        );
        assert!(
            block.lines.iter().all(|row| !row.text.contains('\r')
                && !row.text.contains('\n')),
            "texts carry no terminators: {:?}",
            block.lines
        );
    }

    #[test]
    fn cross_note_first_touch_order_with_revisit_and_second_parent() {
        // Reverse lexical touch order (zulu before alpha), a revisit of the
        // first parent, and a first touch of a second parent in the already
        // touched zulu note. Output must follow first-touch order, not path
        // order, with cumulative added rows and deduplicated roles.
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = write_vault(
            &dir,
            &[
                (
                    "zulu.md",
                    concat!(
                        "- [ ] #task Zulu ^z\n",
                        "\t- keep z\n",
                        "- [ ] #task Zulu Two ^z2\n",
                        "\t- keep z2\n",
                    ),
                ),
                ("alpha.md", "- [ ] #task Alpha ^a\n\t- keep a\n"),
            ],
        );
        let batch = plan(
            &vault,
            "first @zulu+z\n\nsecond @alpha+a\n\nthird @zulu+z\n\nfourth @zulu+z2",
        );
        assert_eq!(batch.task_blocks.len(), 3);
        let ids = batch
            .task_blocks
            .iter()
            .map(|block| block.block_id.as_deref().unwrap_or("<none>"))
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["z", "a", "z2"]);
        for block in &batch.task_blocks {
            assert_eq!(
                block.roles,
                vec![TaskBlockRole::SubBullet],
                "roles deduplicated for {:?}",
                block.block_id
            );
        }
        // Parents stay at their final headline lines: zulu and alpha open
        // their notes; the second zulu parent shifts below zulu's additions.
        assert_eq!(batch.task_blocks[0].line, 1);
        assert_eq!(batch.task_blocks[1].line, 1);
        assert!(batch.task_blocks[2].line > 1);
        let zulu_added = batch.task_blocks[0]
            .lines
            .iter()
            .filter(|row| row.change == BlockLineChange::Added)
            .map(|row| row.text.clone())
            .collect::<Vec<_>>();
        assert!(
            zulu_added.iter().any(|text| text.contains("first"))
                && zulu_added.iter().any(|text| text.contains("third")),
            "zulu block is cumulative: {zulu_added:?}"
        );
        let alpha_added = batch.task_blocks[1]
            .lines
            .iter()
            .filter(|row| row.change == BlockLineChange::Added)
            .map(|row| row.text.clone())
            .collect::<Vec<_>>();
        assert_eq!(alpha_added.len(), 1);
        assert!(alpha_added[0].contains("second"));
        let zulu_two_added = batch.task_blocks[2]
            .lines
            .iter()
            .filter(|row| row.change == BlockLineChange::Added)
            .map(|row| row.text.clone())
            .collect::<Vec<_>>();
        assert_eq!(zulu_two_added.len(), 1);
        assert!(zulu_two_added[0].contains("fourth"));
    }

    #[test]
    fn interior_blank_lines_are_kept_in_the_block() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = write_vault(
            &dir,
            &[(
                "sase.md",
                concat!(
                    "- [ ] #task Parent ^p1\n",
                    "\t- first\n",
                    "\n",
                    "\t- second\n",
                ),
            )],
        );
        let batch = plan(&vault, "new @sase+p1");
        assert_eq!(batch.task_blocks.len(), 1);
        let block = &batch.task_blocks[0];
        assert_eq!(
            row_tuples(block),
            vec![
                ("- [ ] #task Parent ^p1", 0, BlockLineChange::Unchanged),
                ("\t- first", 1, BlockLineChange::Unchanged),
                ("", 0, BlockLineChange::Unchanged),
                ("\t- second", 1, BlockLineChange::Unchanged),
                ("\t- new", 1, BlockLineChange::Added),
            ]
        );
    }
}

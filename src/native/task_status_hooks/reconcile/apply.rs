use super::super::*;
use super::fields::{is_cancel_log_line, leading_bytes};
use super::*;

impl ReconcileWorker<'_> {
    /// Record a task-line rewrite, merging with any pending stamp
    /// or projection already queued for the same line.
    pub(super) fn replace_task_line(
        &mut self,
        view: &TaskView<'_>,
        file_index: usize,
        line_index: usize,
        new_line: &str,
        id: Option<String>,
        depends_on: Vec<String>,
    ) {
        let _ = view;
        self.task_rewrites.insert(
            (file_index, line_index),
            (new_line.to_string(), id, depends_on),
        );
        self.outcome.touched_files.insert(file_index);
    }

    /// The pending `[id::]` of a task line: its queued rewrite's id
    /// when one exists, else the scanned id. Every task-line rewrite
    /// starts from the pending values so chained edits settle in one
    /// run instead of overwriting each other.
    pub(super) fn pending_or_scanned_id(
        &self,
        file_index: usize,
        line_index: usize,
        scanned: Option<String>,
    ) -> Option<String> {
        match self.task_rewrites.get(&(file_index, line_index)) {
            Some((_, id, _)) => id.clone(),
            None => scanned,
        }
    }

    /// The pending `[dependsOn::]` of a task line: its queued
    /// rewrite's ids when one exists, else the scanned ids.
    pub(super) fn pending_or_scanned_depends(
        &self,
        file_index: usize,
        line_index: usize,
        scanned: Vec<String>,
    ) -> Vec<String> {
        match self.task_rewrites.get(&(file_index, line_index)) {
            Some((_, _, depends)) => depends.clone(),
            None => scanned,
        }
    }

    /// The current text of a task line: its pending rewrite when one
    /// is queued, else the snapshot.
    pub(super) fn pending_or_snapshot_line(
        &self,
        file_index: usize,
        line_index: usize,
    ) -> String {
        self.task_rewrites
            .get(&(file_index, line_index))
            .map(|(line, _, _)| line.clone())
            .unwrap_or_else(|| {
                self.snapshots[file_index].lines[line_index].clone()
            })
    }

    pub(super) fn replace_line(
        &mut self,
        file_index: usize,
        line_index: usize,
        new_line: &str,
    ) {
        self.edits.push(PendingEdit {
            file_index,
            line_index,
            kind: EditKind::Replace,
            new_line: new_line.to_string(),
        });
        self.outcome.touched_files.insert(file_index);
    }

    pub(super) fn remove_line(&mut self, file_index: usize, line_index: usize) {
        self.edits.push(PendingEdit {
            file_index,
            line_index,
            kind: EditKind::Remove,
            new_line: String::new(),
        });
        self.outcome.touched_files.insert(file_index);
    }

    pub(super) fn insert_line(
        &mut self,
        file_index: usize,
        line_index: usize,
        new_line: &str,
    ) {
        self.edits.push(PendingEdit {
            file_index,
            line_index,
            kind: EditKind::Insert,
            new_line: new_line.to_string(),
        });
        self.outcome.touched_files.insert(file_index);
    }

    /// Insertion slot for an adopted line: before the first direct
    /// child — or before the second when a `❌ **CANCEL LOG**` child
    /// holds the first slot — with the existing child indent, else
    /// the parent indent plus one tab. `reuse` names a label-only
    /// line whose own indent is reused.
    pub(super) fn child_slot(
        &self,
        view: &TaskView<'_>,
        reuse: Option<usize>,
    ) -> (usize, String) {
        let snap = &self.snapshots[view.file_index];
        if let Some(child_index) = reuse {
            let indent = leading_bytes(snap.lines[child_index].as_str());
            return (child_index, indent.to_string());
        }
        let refs: Vec<&str> = snap.lines.iter().map(String::as_str).collect();
        let parent_indent = leading_bytes(refs[view.task.line_index]);
        let parent_width =
            leading_indentation_width(refs[view.task.line_index]);
        let mut first_child: Option<(usize, String, bool)> = None;
        for line_index in view.task.line_index + 1..refs.len() {
            let line = refs[line_index];
            if line.trim().is_empty() || snap.fenced.contains(&line_index) {
                continue;
            }
            if leading_indentation_width(line) <= parent_width {
                break;
            }
            if nearest_parent_list_item(&refs, line_index)
                != Some(view.task.line_index)
            {
                continue;
            }
            let cancel = is_cancel_log_line(line);
            if first_child.is_none() {
                first_child =
                    Some((line_index, leading_bytes(line).to_string(), cancel));
                if !cancel {
                    break;
                }
                continue;
            }
            // A second direct child after the Cancel Log: insert here.
            return (line_index, leading_bytes(line).to_string());
        }
        // The shared writer indent: the existing child indent when
        // the task has one, else the parent indent plus one tab.
        let existing =
            first_child.as_ref().map(|(_, indent, _)| indent.as_str());
        let indent = task_dependencies::format::child_indent_for_parent(
            parent_indent,
            existing,
        );
        match first_child {
            Some((line_index, _, true)) => (line_index + 1, indent),
            Some((line_index, _, false)) => (line_index, indent),
            None => (view.task.line_index + 1, indent),
        }
    }

    pub(super) fn warn(
        &mut self,
        view: &TaskView<'_>,
        kind: &str,
        detail: &str,
    ) {
        self.outcome.warnings.push(DependencyWarning {
            kind: kind.to_string(),
            path: display_path(view.relative_path),
            line: view.task.line_index + 1,
            detail: detail.to_string(),
        });
    }

    /// Materialise every queued edit into note contents. Child-line
    /// edits apply descending so original indices stay valid;
    /// task-line rewrites merge by line. Returns the report. Only
    /// touched files are cloned for the caller; notes without tasks or
    /// block ids never get line views, so the whole vault is never
    /// copied per run.
    pub(super) fn apply(mut self, files: &mut [FileScan]) -> ReconcileOutcome {
        for ((file_index, line_index), (new_line, _, _)) in &self.task_rewrites
        {
            self.edits.push(PendingEdit {
                file_index: *file_index,
                line_index: *line_index,
                kind: EditKind::Replace,
                new_line: new_line.clone(),
            });
        }
        let mut by_file: BTreeMap<usize, Vec<&PendingEdit>> = BTreeMap::new();
        for edit in &self.edits {
            by_file.entry(edit.file_index).or_default().push(edit);
        }
        for (file_index, mut edits) in by_file {
            // Descending by line so original indices stay valid. At the
            // same original index a Replace/Remove applies before an
            // Insert, so the insert lands before the rewritten original
            // line instead of being overwritten by it.
            self.outcome
                .original_contents
                .insert(file_index, files[file_index].contents.clone());
            edits.sort_by(|left, right| {
                right
                    .line_index
                    .cmp(&left.line_index)
                    .then((left.kind as u8).cmp(&(right.kind as u8)))
            });
            let snap = &self.snapshots[file_index];
            let mut lines = snap.lines.clone();
            for edit in edits {
                match edit.kind {
                    EditKind::Replace => {
                        lines[edit.line_index] = edit.new_line.clone();
                    }
                    EditKind::Remove => {
                        lines.remove(edit.line_index);
                    }
                    EditKind::Insert => {
                        lines.insert(edit.line_index, edit.new_line.clone());
                    }
                }
            }
            let joined = lines.join(snap.line_ending);
            files[file_index].contents = if snap.final_newline {
                format!("{joined}{}", snap.line_ending)
            } else {
                joined
            };
        }
        // Deterministic report order: by note, then line.
        self.outcome.projection_updates.sort_by(|left, right| {
            (&left.path, left.line).cmp(&(&right.path, right.line))
        });
        self.outcome.adopted.sort_by(|left, right| {
            (&left.path, left.line).cmp(&(&right.path, right.line))
        });
        self.outcome.healed.sort_by(|left, right| {
            (&left.path, left.line).cmp(&(&right.path, right.line))
        });
        self.outcome.canonicalized.sort_by(|left, right| {
            (&left.path, left.line).cmp(&(&right.path, right.line))
        });
        self.outcome.warnings.sort_by(|left, right| {
            (&left.kind, &left.path, left.line).cmp(&(
                &right.kind,
                &right.path,
                right.line,
            ))
        });
        self.outcome
    }

    pub(super) fn warn_cycle(
        &mut self,
        files: &[FileScan],
        start: &(PathBuf, String),
        stack: &[(PathBuf, String)],
    ) {
        // Rotate the discovered walk so the path starts at the
        // smallest member; the walk already ends back at `start`.
        let mut ordered = stack.to_vec();
        ordered.push(start.clone());
        let first = ordered.iter().min().cloned().unwrap_or(start.clone());
        let position =
            ordered.iter().position(|node| *node == first).unwrap_or(0);
        let mut rotated = ordered[position..].to_vec();
        rotated.extend_from_slice(&ordered[1..position + 1]);
        let path = rotated
            .iter()
            .map(|(path, block)| format!("{}#^{}", display_path(path), block))
            .collect::<Vec<_>>()
            .join(" -> ");
        let (file_index, line_index) =
            self.graph_nodes.get(&first).copied().unwrap_or((0, 0));
        let relative = files
            .get(file_index)
            .map(|file| display_path(&file.relative_path))
            .unwrap_or_default();
        self.outcome.warnings.push(DependencyWarning {
            kind: "dependency_cycle".to_string(),
            path: relative,
            line: line_index + 1,
            detail: format!("dependency cycle: {path}"),
        });
    }
}

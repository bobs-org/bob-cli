//! Linked-task resolution and close-plan composition.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use chrono::NaiveDateTime;

use super::super::{
    capture::{line_spans, LineSpan},
    capture_language::{self, CloseLogEntry},
    capture_task_toggle, capture_work_log,
    note_tasks::{
        self, BlockIdLookup, NoteTask, NoteTaskSettings, TaskStatusType,
    },
    pomodoro,
    vault_links::LinkResolution,
};
use super::ledger::target_from_token;
use super::links::{apply_edits, pomodoro_marker_prefix};
use super::selection::{
    apply_close_selection, number_task_links, CloseSelection,
    CloseSelectionError, NumberedTaskLink, TaskLinkOutcome, TaskLinkSource,
};
use super::{
    close_task_text, find_running_pomodoro, plan_ledger_close,
    plan_ledger_close_with_parked, sub_bullet_range, wikilink_tokens,
    BlockLinkTarget, FindRunningError, LedgerClosePlan, LedgerLinkRole,
    RunningPomodoro, WorkLogNode,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloseTaskRole {
    Worked,
    Mentioned,
    Deferred,
    Dropped,
    Struck,
    Embedded,
    Subtask,
}

impl CloseTaskRole {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Worked => "worked",
            Self::Mentioned => "mentioned",
            Self::Deferred => "deferred",
            Self::Dropped => "dropped",
            Self::Struck => "struck",
            Self::Embedded => "embedded",
            Self::Subtask => "subtask",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PomodoroCloseTask {
    pub role: CloseTaskRole,
    pub block_link: String,
    pub ledger_line: usize,
    pub resolved: bool,
    pub relative_target: Option<String>,
    pub block_id: String,
    pub text: Option<String>,
    pub previous_status_symbol: Option<char>,
    pub previous_status_name: Option<String>,
    pub status_symbol: Option<char>,
    pub status_name: Option<String>,
    pub status_changed: bool,
    pub carried: bool,
    pub work_log: Vec<String>,
    pub work_log_created: bool,
    /// Dated entries this close's typed Work Log entries produced, in typed
    /// order (a subset of `work_log`).
    pub typed_work_log: Vec<String>,
    /// Detail lines written under each typed entry, aligned 1:1 with
    /// `typed_work_log`; entries without details hold an empty list.
    pub typed_work_log_details: Vec<Vec<String>>,
    pub warning: Option<String>,
    pub index: Option<u32>,
    resolved_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PomodoroCloseSummary {
    pub running: RunningPomodoro,
    pub ledger: LedgerClosePlan,
    pub tasks: Vec<PomodoroCloseTask>,
    pub task_links: Vec<NumberedTaskLink>,
    /// Typed Work Log entries in typed order with every positional index
    /// resolved, so `bob capture` JSON reports the task each entry logged
    /// to.
    pub log: Vec<CloseLogEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PomodoroClosePlan {
    /// Post-images for changed files only. The day note may also be a task note.
    pub changed_files: BTreeMap<PathBuf, String>,
    pub summary: PomodoroCloseSummary,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PomodoroClosePlanError {
    FindRunning(FindRunningError),
    VaultRead(String),
    Selection(CloseSelectionError),
}

/// Read-only access to the vault view used by a close plan. Implementations
/// resolve against the day file and return the latest staged content, so a
/// batch planner can compose edits without writing any files.
pub(crate) trait CloseVault {
    fn bob_dir(&self) -> &Path;
    fn resolve_target(&self, from_path: &Path, target: &str) -> LinkResolution;
    fn read_latest(&self, path: &Path) -> Result<Option<String>, String>;
}

type TaskKey = (PathBuf, String);

/// Identity of one close task row: where its link was found and how the
/// row reports. Groups the `register_reference` arguments.
struct CloseTaskRef<'a> {
    from_path: &'a Path,
    path_part: &'a str,
    block_id: &'a str,
    role: CloseTaskRole,
    block_link: &'a str,
    ledger_line: usize,
    carried: bool,
}

struct ClosePlanner<'a, V> {
    vault: &'a V,
    day_path: &'a Path,
    settings: NoteTaskSettings,
    all_task_settings: NoteTaskSettings,
    date: String,
    staged: BTreeMap<PathBuf, String>,
    originals: BTreeMap<PathBuf, String>,
    task_indices: BTreeMap<TaskKey, usize>,
    unresolved_indices: BTreeMap<String, usize>,
    visited_embeds: BTreeSet<TaskKey>,
    visited_embed_count: usize,
    tasks: Vec<PomodoroCloseTask>,
    warnings: Vec<String>,
}

impl<'a, V: CloseVault> ClosePlanner<'a, V> {
    fn new(
        vault: &'a V,
        day_path: &'a Path,
        day_contents: &str,
        initial_ledger: &LedgerClosePlan,
        now: NaiveDateTime,
    ) -> Self {
        let settings = note_tasks::read_settings(vault.bob_dir());
        let mut all_task_settings = settings.clone();
        all_task_settings.global_filter.clear();
        let mut staged = BTreeMap::new();
        staged.insert(day_path.to_path_buf(), initial_ledger.contents.clone());
        let mut originals = BTreeMap::new();
        originals.insert(day_path.to_path_buf(), day_contents.to_string());
        let date = day_path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(pomodoro::parse_day_file_date)
            .unwrap_or_else(|| now.date())
            .format("%Y-%m-%d")
            .to_string();
        Self {
            vault,
            day_path,
            settings,
            all_task_settings,
            date,
            staged,
            originals,
            task_indices: BTreeMap::new(),
            unresolved_indices: BTreeMap::new(),
            visited_embeds: BTreeSet::new(),
            visited_embed_count: 0,
            tasks: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn read_file(
        &mut self,
        path: &Path,
    ) -> Result<Option<String>, PomodoroClosePlanError> {
        if let Some(contents) = self.staged.get(path) {
            return Ok(Some(contents.clone()));
        }
        let contents = self
            .vault
            .read_latest(path)
            .map_err(PomodoroClosePlanError::VaultRead)?;
        if let Some(contents) = contents {
            self.originals.insert(path.to_path_buf(), contents.clone());
            self.staged.insert(path.to_path_buf(), contents.clone());
            Ok(Some(contents))
        } else {
            Ok(None)
        }
    }

    fn save_file(&mut self, path: &Path, contents: String) {
        self.staged.insert(path.to_path_buf(), contents);
    }

    fn warn(&mut self, message: String) {
        if !self.warnings.contains(&message) {
            self.warnings.push(message);
        }
    }

    fn row_warning(&mut self, index: usize, message: String) {
        self.warn(message.clone());
        if let Some(task) = self.tasks.get_mut(index)
            && task.warning.is_none()
        {
            task.warning = Some(message);
        }
    }

    fn relative_target(&self, path: &Path) -> String {
        path.strip_prefix(self.vault.bob_dir())
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    fn register_reference(
        &mut self,
        target: CloseTaskRef<'_>,
        allow_non_task: bool,
    ) -> Result<Option<TaskKey>, PomodoroClosePlanError> {
        let path = match self
            .vault
            .resolve_target(target.from_path, target.path_part)
        {
            LinkResolution::Found(path) => path,
            LinkResolution::Ambiguous => {
                let warning = format!(
                        "{} has an ambiguous note basename; the target was left unchanged",
                        target.block_link
                    );
                self.register_unresolved(
                    target.path_part,
                    target.block_id,
                    target.role,
                    target.block_link,
                    target.ledger_line,
                    target.carried,
                    None,
                    warning,
                );
                return Ok(None);
            }
            LinkResolution::Missing => {
                let warning = format!(
                        "{} does not resolve to a vault note; the target was left unchanged",
                        target.block_link
                    );
                self.register_unresolved(
                    target.path_part,
                    target.block_id,
                    target.role,
                    target.block_link,
                    target.ledger_line,
                    target.carried,
                    None,
                    warning,
                );
                return Ok(None);
            }
        };
        let key = (path.clone(), target.block_id.to_string());
        if let Some(index) = self.task_indices.get(&key).copied() {
            if let Some(task) = self.tasks.get_mut(index) {
                task.carried |= target.carried;
            }
            return Ok(self.tasks[index].resolved.then_some(key));
        }

        let relative_target = self.relative_target(&path);
        let contents = self.read_file(&path)?;
        let Some(contents) = contents else {
            let warning = format!(
                "{} resolves to {relative_target}, which cannot be read; the target was left unchanged",
                target.block_link
            );
            self.register_unresolved(
                target.path_part,
                target.block_id,
                target.role,
                target.block_link,
                target.ledger_line,
                target.carried,
                Some(path),
                warning,
            );
            return Ok(None);
        };
        let settings = if allow_non_task {
            &self.all_task_settings
        } else {
            &self.settings
        };
        match lookup_task(&contents, settings, target.block_id) {
            Ok(note_task) => {
                let index = self.tasks.len();
                self.tasks.push(PomodoroCloseTask {
                    role: target.role,
                    block_link: target.block_link.to_string(),
                    ledger_line: target.ledger_line,
                    resolved: true,
                    relative_target: Some(relative_target),
                    block_id: target.block_id.to_string(),
                    text: Some(close_task_text(&note_task.description)),
                    previous_status_symbol: Some(note_task.status_symbol),
                    previous_status_name: Some(note_task.status_name.clone()),
                    status_symbol: Some(note_task.status_symbol),
                    status_name: Some(note_task.status_name.clone()),
                    status_changed: false,
                    carried: target.carried,
                    work_log: Vec::new(),
                    work_log_created: false,
                    typed_work_log: Vec::new(),
                    typed_work_log_details: Vec::new(),
                    warning: None,
                    index: None,
                    resolved_path: Some(path.clone()),
                });
                self.task_indices.insert(key.clone(), index);
                Ok(Some(key))
            }
            Err(reason) => {
                let warning = format!(
                    "{} in {relative_target} {reason}; the target was left unchanged",
                    target.block_link
                );
                let index = self.tasks.len();
                self.tasks.push(PomodoroCloseTask {
                    role: target.role,
                    block_link: target.block_link.to_string(),
                    ledger_line: target.ledger_line,
                    resolved: false,
                    relative_target: Some(relative_target),
                    block_id: target.block_id.to_string(),
                    text: None,
                    previous_status_symbol: None,
                    previous_status_name: None,
                    status_symbol: None,
                    status_name: None,
                    status_changed: false,
                    carried: target.carried,
                    work_log: Vec::new(),
                    work_log_created: false,
                    typed_work_log: Vec::new(),
                    typed_work_log_details: Vec::new(),
                    warning: Some(warning.clone()),
                    index: None,
                    resolved_path: Some(path.clone()),
                });
                self.task_indices.insert(key, index);
                self.warn(warning);
                Ok(None)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn register_unresolved(
        &mut self,
        path_part: &str,
        block_id: &str,
        role: CloseTaskRole,
        block_link: &str,
        ledger_line: usize,
        carried: bool,
        resolved_path: Option<PathBuf>,
        warning: String,
    ) {
        let key = format!("{path_part}#^{block_id}");
        if let Some(index) = self.unresolved_indices.get(&key).copied() {
            if let Some(task) = self.tasks.get_mut(index) {
                task.carried |= carried;
            }
            return;
        }
        let index = self.tasks.len();
        self.tasks.push(PomodoroCloseTask {
            role,
            block_link: block_link.to_string(),
            ledger_line,
            resolved: false,
            relative_target: resolved_path
                .as_deref()
                .map(|path| self.relative_target(path)),
            block_id: block_id.to_string(),
            text: None,
            previous_status_symbol: None,
            previous_status_name: None,
            status_symbol: None,
            status_name: None,
            status_changed: false,
            carried,
            work_log: Vec::new(),
            work_log_created: false,
            typed_work_log: Vec::new(),
            typed_work_log_details: Vec::new(),
            warning: Some(warning.clone()),
            index: None,
            resolved_path,
        });
        self.unresolved_indices.insert(key, index);
        self.warn(warning);
    }

    fn task_for_key(
        &mut self,
        key: &TaskKey,
        allow_non_task: bool,
    ) -> Result<Option<NoteTask>, PomodoroClosePlanError> {
        let Some(contents) = self.read_file(&key.0)? else {
            return Ok(None);
        };
        let settings = if allow_non_task {
            &self.all_task_settings
        } else {
            &self.settings
        };
        Ok(lookup_task(&contents, settings, &key.1).ok())
    }

    fn replace_task_line(
        &mut self,
        key: &TaskKey,
        task: &NoteTask,
        replacement: String,
    ) -> Result<bool, PomodoroClosePlanError> {
        let Some(contents) = self.read_file(&key.0)? else {
            return Ok(false);
        };
        let Some(updated) =
            replace_line(&contents, task.line_index, &replacement)
        else {
            return Ok(false);
        };
        if contents == updated {
            return Ok(false);
        }
        self.save_file(&key.0, updated);
        Ok(true)
    }

    fn refresh_task_row(
        &mut self,
        key: &TaskKey,
        allow_non_task: bool,
    ) -> Result<(), PomodoroClosePlanError> {
        let task = self.task_for_key(key, allow_non_task)?;
        let Some(index) = self.task_indices.get(key).copied() else {
            return Ok(());
        };
        let Some(task) = task else {
            return Ok(());
        };
        if let Some(row) = self.tasks.get_mut(index) {
            row.resolved = true;
            row.text = Some(close_task_text(&task.description));
            row.status_symbol = Some(task.status_symbol);
            row.status_name = Some(task.status_name.clone());
            row.status_changed =
                row.previous_status_symbol != row.status_symbol;
        }
        Ok(())
    }

    fn apply_startable(
        &mut self,
        target: &BlockLinkTarget,
        carried: bool,
    ) -> Result<(), PomodoroClosePlanError> {
        let link = &target.wikilink;
        let Some(key) = self.register_reference(
            CloseTaskRef {
                from_path: self.day_path,
                path_part: &target.path_part,
                block_id: &target.block_id,
                role: CloseTaskRole::Worked,
                block_link: link,
                ledger_line: target.line,
                carried,
            },
            false,
        )?
        else {
            return Ok(());
        };
        let Some(task) = self.task_for_key(&key, false)? else {
            return Ok(());
        };
        if !matches!(task.status_symbol, ' ' | '*') {
            return Ok(());
        }
        let Some(line) = line_text_at(&self.staged[&key.0], task.line_index)
        else {
            return Ok(());
        };
        let Some(updated) =
            capture_task_toggle::set_task_line_status(line, '/')
        else {
            return Ok(());
        };
        let updated = normalize_task_metadata_spacing(&updated);
        // Freshness: stamp the `[/]` close as the last transformation.
        // A refusal (recurring/closed) leaves the line as it is.
        let updated =
            match chrono::NaiveDate::parse_from_str(&self.date, "%Y-%m-%d") {
                Ok(close_date) => {
                    let stamped = super::super::freshness::stamp_fresh(
                        &updated, close_date,
                    );
                    if stamped.refused.is_none() {
                        stamped.line
                    } else {
                        updated
                    }
                }
                Err(_) => updated,
            };
        if self.replace_task_line(&key, &task, updated)? {
            self.refresh_task_row(&key, false)?;
        }
        Ok(())
    }

    fn apply_embedded_tree(
        &mut self,
        from_path: &Path,
        target: &BlockLinkTarget,
        depth: usize,
        ledger_line: usize,
        role: CloseTaskRole,
    ) -> Result<(), PomodoroClosePlanError> {
        if depth > 25 {
            self.warn(format!(
                "embedded task recursion exceeded 25 levels below line {ledger_line}"
            ));
            return Ok(());
        }
        let Some(key) = self.register_reference(
            CloseTaskRef {
                from_path,
                path_part: &target.path_part,
                block_id: &target.block_id,
                role,
                block_link: &target.wikilink,
                ledger_line,
                carried: false,
            },
            true,
        )?
        else {
            return Ok(());
        };
        if self.visited_embeds.contains(&key) {
            return Ok(());
        }
        if self.visited_embed_count >= 250 {
            self.warn(
                "embedded task recursion exceeded 250 targets".to_string(),
            );
            return Ok(());
        }
        self.visited_embeds.insert(key.clone());
        self.visited_embed_count += 1;

        let Some(task) = self.task_for_key(&key, true)? else {
            return Ok(());
        };
        if !matches!(task.status_symbol, ' ' | '*' | '/' | 'x') {
            return Ok(());
        }
        let Some(contents) = self.read_file(&key.0)? else {
            return Ok(());
        };
        let descendants = embedded_children(&contents, &task);
        for descendant in descendants {
            self.apply_embedded_tree(
                &key.0,
                &descendant,
                depth + 1,
                ledger_line,
                CloseTaskRole::Subtask,
            )?;
        }

        let Some(current_task) = self.task_for_key(&key, true)? else {
            return Ok(());
        };
        if matches!(current_task.status_symbol, ' ' | '*' | '/') {
            let Some(line) = self.staged.get(&key.0).and_then(|contents| {
                line_text_at(contents, current_task.line_index)
            }) else {
                return Ok(());
            };
            let Some(mut updated) =
                capture_task_toggle::set_task_line_status(line, 'x')
            else {
                return Ok(());
            };
            if task_has_global_filter(line, &self.settings) {
                updated = add_or_replace_completion_field(&updated, &self.date);
            }
            if self.replace_task_line(&key, &current_task, updated)? {
                self.refresh_task_row(&key, true)?;
            }
        }
        Ok(())
    }

    fn write_logs(
        &mut self,
        ledger: &LedgerClosePlan,
        log_lines: &BTreeMap<usize, usize>,
        log_entries: &[CloseLogEntry],
    ) -> Result<BTreeSet<usize>, PomodoroClosePlanError> {
        let mut groups: Vec<(
            TaskKey,
            usize,
            Vec<Vec<capture_work_log::WorkLogNode>>,
        )> = Vec::new();
        let mut group_indices = BTreeMap::<TaskKey, usize>::new();
        for group in &ledger.work_log_groups {
            let block_link =
                format!("[[{}#^{}]]", group.path_part, group.block_id);
            let (role, carried) = ledger
                .classified_links
                .iter()
                .find(|link| {
                    link.line == group.source_line
                        && link.path_part == group.path_part
                        && link.block_id == group.block_id
                })
                .map(|link| (close_role(link.role), link.carried))
                .unwrap_or((CloseTaskRole::Mentioned, false));
            // Dropped links take no task effect: no start and no Work Log.
            if role == CloseTaskRole::Dropped {
                continue;
            }
            let Some(key) = self.register_reference(
                CloseTaskRef {
                    from_path: self.day_path,
                    path_part: &group.path_part,
                    block_id: &group.block_id,
                    role,
                    block_link: &block_link,
                    ledger_line: group.source_line,
                    carried,
                },
                false,
            )?
            else {
                continue;
            };
            let index =
                *group_indices.entry(key.clone()).or_insert_with(|| {
                    let index = groups.len();
                    groups.push((key.clone(), group.source_line, Vec::new()));
                    index
                });
            groups[index].2.push(
                group.descendant_roots.iter().map(work_log_node).collect(),
            );
        }

        let mut landed = BTreeSet::new();
        let mut typed_by_row: BTreeMap<usize, Vec<(usize, String)>> =
            BTreeMap::new();
        for (key, _first_source_line, note_groups) in groups {
            let mut cursor = None;
            for note_roots in note_groups {
                let Some(contents) = self.read_file(&key.0)? else {
                    continue;
                };
                let Some(task) =
                    lookup_task(&contents, &self.settings, &key.1).ok()
                else {
                    continue;
                };
                let Some(write) = capture_work_log::write_work_log_group(
                    &contents,
                    task.line_index,
                    &note_roots,
                    &self.date,
                    cursor.as_ref(),
                ) else {
                    continue;
                };
                self.save_file(&key.0, write.contents);
                if let Some(index) = self.task_indices.get(&key).copied()
                    && let Some(row) = self.tasks.get_mut(index)
                {
                    for (dated, source) in
                        write.entries.iter().zip(write.entry_sources.iter())
                    {
                        row.work_log.push(dated.clone());
                        if let Some(source) = source
                            && let Some(ordinal) = log_lines.get(source)
                        {
                            typed_by_row
                                .entry(index)
                                .or_default()
                                .push((*ordinal, dated.clone()));
                            landed.insert(*ordinal);
                        }
                    }
                    row.work_log_created = true;
                }
                cursor = Some(write.next_cursor);
            }
        }
        for (index, mut pairs) in typed_by_row {
            pairs.sort();
            if let Some(row) = self.tasks.get_mut(index) {
                // The descendants written under an inserted entry are
                // exactly its typed details: the line after the insertion
                // point is never deeper than the link.
                row.typed_work_log_details = pairs
                    .iter()
                    .map(|(ordinal, _)| {
                        log_entries
                            .get(*ordinal)
                            .map(|entry| entry.details.clone())
                            .unwrap_or_default()
                    })
                    .collect();
                row.typed_work_log =
                    pairs.into_iter().map(|(_, dated)| dated).collect();
            }
        }
        Ok(landed)
    }

    fn warn_missing_typed_logs(
        &mut self,
        entries: &[CloseLogEntry],
        task_links: &[NumberedTaskLink],
        link_rows: &BTreeMap<u32, usize>,
        landed: &BTreeSet<usize>,
    ) {
        let by_index: BTreeMap<u32, &NumberedTaskLink> =
            task_links.iter().map(|link| (link.index, link)).collect();
        for (ordinal, entry) in entries.iter().enumerate() {
            if landed.contains(&ordinal) {
                continue;
            }
            // Entries resolve before planning, so every index is `Some`.
            let Some(index) = entry.index else {
                continue;
            };
            let block = by_index
                .get(&index)
                .map(|link| link.block_link.as_str())
                .unwrap_or("?");
            let message = format!(
                "task {index} `{block}` has no task line, so its Work Log entry stays only in the Pomodoro",
            );
            if let Some(row) = link_rows.get(&index).copied() {
                self.row_warning(row, message);
            } else {
                self.warn(message);
            }
        }
    }

    fn retire_closed_embeds(
        &mut self,
        ledger: &LedgerClosePlan,
        entry_line_index: usize,
    ) -> Result<(), PomodoroClosePlanError> {
        let Some(closed_entry_line) =
            line_text_at(&ledger.contents, entry_line_index)
        else {
            return Ok(());
        };
        let current_day = self.read_file(self.day_path)?.unwrap_or_default();
        let day_spans = line_spans(&current_day);
        let entry_index = day_spans
            .iter()
            .position(|line| line.text == closed_entry_line);
        let Some(entry_index) = entry_index else {
            return Ok(());
        };
        let day_lines =
            day_spans.iter().map(|line| line.text).collect::<Vec<_>>();
        let range = sub_bullet_range(&day_lines, entry_index);
        let mut retired_by_line =
            BTreeMap::<usize, BTreeSet<(PathBuf, String)>>::new();
        for line_index in range.clone() {
            for token in wikilink_tokens(day_lines[line_index])
                .into_iter()
                .filter(|token| token.embedded)
            {
                let LinkResolution::Found(path) =
                    self.vault.resolve_target(self.day_path, &token.path_part)
                else {
                    continue;
                };
                let key = (path.clone(), token.block_id.clone());
                let Some(task) = self.task_for_key(&key, true)? else {
                    continue;
                };
                if task.status_type == TaskStatusType::Done
                    || task.status_symbol == 'x'
                {
                    retired_by_line.entry(line_index).or_default().insert(key);
                }
            }
        }
        if retired_by_line.is_empty() {
            return Ok(());
        }

        let mut updated = current_day;
        for (line_index, closed_targets) in retired_by_line {
            let Some(line) = line_text_at(&updated, line_index) else {
                continue;
            };
            let rewritten = retire_embedded_links(
                line,
                &closed_targets,
                self.vault,
                self.day_path,
            );
            if rewritten != line
                && let Some(next) =
                    replace_line(&updated, line_index, &rewritten)
            {
                updated = next;
            }
        }
        self.save_file(self.day_path, updated);
        Ok(())
    }
}

pub(crate) fn plan_pomodoro_close<V: CloseVault>(
    day_path: &Path,
    day_contents: &str,
    now: NaiveDateTime,
    vault: &V,
    selection: Option<&CloseSelection>,
) -> Result<PomodoroClosePlan, PomodoroClosePlanError> {
    let running = find_running_pomodoro(day_contents)
        .map_err(PomodoroClosePlanError::FindRunning)?;
    let (working_contents, task_links, inserted_lines, resolved_log) =
        match selection {
            Some(sel) => {
                let applied =
                    apply_close_selection(day_contents, &running, sel)
                        .map_err(PomodoroClosePlanError::Selection)?;
                (
                    applied.contents,
                    applied.lineup,
                    applied.inserted_lines,
                    applied.log,
                )
            }
            None => (
                day_contents.to_string(),
                number_task_links(day_contents, &running),
                Vec::new(),
                Vec::new(),
            ),
        };
    let log_lines: BTreeMap<usize, usize> = inserted_lines
        .iter()
        .enumerate()
        .map(|(ordinal, line)| (*line, ordinal))
        .collect();
    let log_entries: Vec<CloseLogEntry> = resolved_log.clone();
    // Parked source lines after Work Log insertion: explicit carry metadata.
    // Only selected parked lines are suppressed; other independently carried
    // references keep their effects.
    let parked_lines: BTreeSet<usize> = task_links
        .iter()
        .filter(|link| link.outcome == TaskLinkOutcome::Parked)
        .map(|link| link.line)
        .collect();
    check_resolved_park_conflicts(vault, day_path, &task_links)?;
    let mut ledger = if parked_lines.is_empty() {
        plan_ledger_close(&working_contents, &running, now)
    } else {
        plan_ledger_close_with_parked(
            &working_contents,
            &running,
            now,
            &parked_lines,
        )
    };
    let mut planner =
        ClosePlanner::new(vault, day_path, day_contents, &ledger, now);

    for link in &ledger.classified_links {
        let allow_non_task = link.role == LedgerLinkRole::Embedded;
        let _ = planner.register_reference(
            CloseTaskRef {
                from_path: day_path,
                path_part: &link.path_part,
                block_id: &link.block_id,
                role: close_role(link.role),
                block_link: &link.raw_target,
                ledger_line: link.line,
                carried: link.carried,
            },
            allow_non_task,
        )?;
    }
    for target in &ledger.startable_targets {
        let carried = !parked_lines.contains(&target.line);
        planner.apply_startable(target, carried)?;
    }
    for target in &ledger.embedded_targets {
        planner.apply_embedded_tree(
            day_path,
            target,
            0,
            target.line,
            CloseTaskRole::Embedded,
        )?;
    }
    let landed = planner.write_logs(&ledger, &log_lines, &log_entries)?;
    planner.retire_closed_embeds(&ledger, running.line.saturating_sub(1))?;
    let link_rows = link_row_mapping(
        planner.vault,
        planner.day_path,
        &planner.task_indices,
        &planner.unresolved_indices,
        &task_links,
    );
    assign_task_indices(&mut planner.tasks, &task_links, &link_rows);
    emit_listed_status_warnings(&mut planner, &task_links, &link_rows);
    planner.warn_missing_typed_logs(
        &log_entries,
        &task_links,
        &link_rows,
        &landed,
    );

    if let Some(contents) = planner.staged.get(day_path) {
        ledger.contents = contents.clone();
    }
    let changed_files = planner
        .staged
        .iter()
        .filter(|(path, contents)| {
            planner.originals.get(*path) != Some(*contents)
        })
        .map(|(path, contents)| (path.clone(), contents.clone()))
        .collect();
    Ok(PomodoroClosePlan {
        changed_files,
        summary: PomodoroCloseSummary {
            running,
            ledger,
            tasks: planner.tasks,
            task_links,
            log: log_entries,
        },
        warnings: planner.warnings,
    })
}

/// Map every numbered link to the task row that owns its task, by task
/// identity rather than the row's first ledger line. A task gets one row,
/// registered from the first ledger line that mentions it, so an earlier
/// unnumbered (mentioned or struck) line can own the row while a later
/// numbered line carries the task's number.
fn link_row_mapping<V: CloseVault>(
    vault: &V,
    day_path: &Path,
    task_indices: &BTreeMap<TaskKey, usize>,
    unresolved_indices: &BTreeMap<String, usize>,
    task_links: &[NumberedTaskLink],
) -> BTreeMap<u32, usize> {
    let mut mapping = BTreeMap::new();
    for link in task_links {
        let row = match vault.resolve_target(day_path, &link.path_part) {
            LinkResolution::Found(path) => {
                task_indices.get(&(path, link.block_id.clone())).copied()
            }
            LinkResolution::Missing | LinkResolution::Ambiguous => {
                let key = format!("{}#^{}", link.path_part, link.block_id);
                unresolved_indices.get(&key).copied()
            }
        };
        if let Some(row) = row {
            mapping.insert(link.index, row);
        }
    }
    mapping
}

fn assign_task_indices(
    tasks: &mut [PomodoroCloseTask],
    task_links: &[NumberedTaskLink],
    link_rows: &BTreeMap<u32, usize>,
) {
    let mut best_by_row: BTreeMap<usize, u32> = BTreeMap::new();
    for link in task_links {
        if let Some(row) = link_rows.get(&link.index).copied()
            && let Some(current) = best_by_row.get(&row).copied()
        {
            if link.index < current {
                best_by_row.insert(row, link.index);
            }
        } else if let Some(row) = link_rows.get(&link.index).copied() {
            best_by_row.insert(row, link.index);
        }
    }
    for (row, task) in tasks.iter_mut().enumerate() {
        if task.role == CloseTaskRole::Subtask {
            task.index = None;
            continue;
        }
        task.index = best_by_row.get(&row).copied();
    }
}

fn emit_listed_status_warnings<V: CloseVault>(
    planner: &mut ClosePlanner<'_, V>,
    task_links: &[NumberedTaskLink],
    link_rows: &BTreeMap<u32, usize>,
) {
    let mut by_link: BTreeMap<u32, &NumberedTaskLink> = BTreeMap::new();
    for link in task_links {
        if link.source == TaskLinkSource::Listed {
            by_link.insert(link.index, link);
        }
    }
    // Group listed links by row; warn at most once per row using the lowest
    // listed number on that row.
    let mut listed_by_row: BTreeMap<usize, Vec<&NumberedTaskLink>> =
        BTreeMap::new();
    for link in by_link.values() {
        if let Some(row) = link_rows.get(&link.index).copied() {
            listed_by_row.entry(row).or_default().push(*link);
        }
    }
    for (row, links) in listed_by_row {
        let (resolved, resolved_path, block_id, row_status_name) =
            match planner.tasks.get(row) {
                Some(task) => (
                    task.resolved,
                    task.resolved_path.clone(),
                    task.block_id.clone(),
                    task.status_name.clone(),
                ),
                None => continue,
            };
        if !resolved {
            continue;
        }
        let Some(first) = links.iter().min_by_key(|link| link.index) else {
            continue;
        };
        let number = first.index;
        let Some(resolved_path) = resolved_path else {
            continue;
        };
        let key = (resolved_path, block_id);
        let Ok(Some(note_task)) = planner.task_for_key(&key, true) else {
            continue;
        };
        let status_name =
            row_status_name.unwrap_or_else(|| note_task.status_name.clone());
        // Fall back to the refreshed row name when the lookup has none.
        let status_name = if status_name.is_empty() {
            note_task.status_name.clone()
        } else {
            status_name
        };
        match first.outcome {
            super::selection::TaskLinkOutcome::InProgress
            | super::selection::TaskLinkOutcome::Parked
                if note_task.status_type != TaskStatusType::InProgress =>
            {
                planner.row_warning(
                    row,
                    format!(
                        "task {number} `{}` is {status_name}, so it was not started",
                        first.block_link
                    ),
                );
            }
            super::selection::TaskLinkOutcome::Complete
                if note_task.status_type != TaskStatusType::Done
                    && note_task.status_symbol != 'x' =>
            {
                planner.row_warning(
                    row,
                    format!(
                        "task {number} `{}` is {status_name}, so it was not completed",
                        first.block_link
                    ),
                );
            }
            _ => {}
        }
    }
}

/// Resolved-identity duplicate guard for parking: two numbered links that
/// resolve to the same vault task must agree on the entire outcome including
/// carry. Ordinary worked and parked conflict; two parked occurrences are
/// allowed. Only runs when parking is involved, so unrelated existing
/// selections keep their historical merging behavior.
fn check_resolved_park_conflicts<V: CloseVault>(
    vault: &V,
    day_path: &Path,
    task_links: &[NumberedTaskLink],
) -> Result<(), PomodoroClosePlanError> {
    if !task_links
        .iter()
        .any(|link| link.outcome == TaskLinkOutcome::Parked)
    {
        return Ok(());
    }
    let mut by_key: BTreeMap<TaskKey, Vec<&NumberedTaskLink>> = BTreeMap::new();
    for link in task_links {
        let LinkResolution::Found(path) =
            vault.resolve_target(day_path, &link.path_part)
        else {
            continue;
        };
        by_key
            .entry((path, link.block_id.clone()))
            .or_default()
            .push(link);
    }
    for links in by_key.values() {
        if links.len() < 2 {
            continue;
        }
        let first_outcome = links[0].outcome;
        if links.iter().any(|link| link.outcome != first_outcome) {
            let mut indices: Vec<u32> =
                links.iter().map(|link| link.index).collect();
            indices.sort_unstable();
            return Err(PomodoroClosePlanError::Selection(
                CloseSelectionError::ConflictingDuplicate {
                    indices,
                    block_link: links[0].block_link.clone(),
                },
            ));
        }
    }
    Ok(())
}

fn close_role(role: LedgerLinkRole) -> CloseTaskRole {
    match role {
        LedgerLinkRole::Worked => CloseTaskRole::Worked,
        LedgerLinkRole::Mentioned => CloseTaskRole::Mentioned,
        LedgerLinkRole::Deferred => CloseTaskRole::Deferred,
        LedgerLinkRole::Dropped => CloseTaskRole::Dropped,
        LedgerLinkRole::Struck => CloseTaskRole::Struck,
        LedgerLinkRole::Embedded => CloseTaskRole::Embedded,
    }
}

pub(crate) fn lookup_task(
    contents: &str,
    settings: &NoteTaskSettings,
    block_id: &str,
) -> Result<NoteTask, String> {
    let scan = note_tasks::scan(contents, settings);
    match scan.by_block_id(block_id) {
        BlockIdLookup::Found(task) => Ok(task.clone()),
        BlockIdLookup::NotATask {
            line_index,
            excerpt,
        } => Err(format!(
            "contains ^{block_id} on a non-task line {} ({excerpt})",
            line_index + 1
        )),
        BlockIdLookup::Duplicate(count) => Err(format!(
            "contains duplicate ^{block_id} IDs ({count} lines)"
        )),
        BlockIdLookup::Missing => {
            Err(format!("has no task with block ID ^{block_id}"))
        }
    }
}

fn embedded_children(contents: &str, task: &NoteTask) -> Vec<BlockLinkTarget> {
    let spans = line_spans(contents);
    let mut children = Vec::new();
    for (index, span) in spans.iter().enumerate().skip(task.line_index + 1) {
        if span.end > task.block_end {
            break;
        }
        children.extend(
            wikilink_tokens(span.text)
                .iter()
                .filter(|token| token.embedded)
                .map(|token| target_from_token(index + 1, token)),
        );
    }
    children
}

fn work_log_node(node: &WorkLogNode) -> capture_work_log::WorkLogNode {
    capture_work_log::WorkLogNode {
        marker: node.marker.clone(),
        body_text: node.body_text.clone(),
        children: node.children.iter().map(work_log_node).collect(),
        source_line: node.source_line,
    }
}

fn line_start(spans: &[LineSpan<'_>], line_index: usize) -> Option<usize> {
    if line_index >= spans.len() {
        return None;
    }
    Some(if line_index == 0 {
        0
    } else {
        spans[line_index - 1].end
    })
}

fn line_text_at(contents: &str, line_index: usize) -> Option<&str> {
    line_spans(contents).get(line_index).map(|line| line.text)
}

fn replace_line(
    contents: &str,
    line_index: usize,
    replacement: &str,
) -> Option<String> {
    let spans = line_spans(contents);
    let span = spans.get(line_index)?;
    let start = line_start(&spans, line_index)?;
    let end = start.checked_add(span.text.len())?;
    let mut output = String::with_capacity(contents.len() + replacement.len());
    output.push_str(&contents[..start]);
    output.push_str(replacement);
    output.push_str(&contents[end..]);
    Some(output)
}

fn normalize_task_metadata_spacing(line: &str) -> String {
    let line = line.trim_end_matches([' ', '\t']);
    let Some((before, last)) = line.rsplit_once(char::is_whitespace) else {
        return line.to_string();
    };
    let id = last.strip_prefix('^').unwrap_or("");
    if !capture_language::is_block_id(id) {
        return line.to_string();
    }
    format!("{} ^{id}", before.trim_end_matches([' ', '\t']))
}

fn task_has_global_filter(line: &str, settings: &NoteTaskSettings) -> bool {
    !settings.global_filter.is_empty()
        && line
            .split_whitespace()
            .any(|token| token == settings.global_filter)
}

fn remove_completion_fields(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut ranges = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = line[cursor..].find("[completion::") {
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
    let mut updated = line.to_string();
    for (start, end) in ranges.into_iter().rev() {
        updated.replace_range(start..end, "");
    }
    normalize_task_metadata_spacing(&updated)
}

fn add_or_replace_completion_field(line: &str, date: &str) -> String {
    let cleaned = remove_completion_fields(line);
    let completion = format!("[completion:: {date}]");
    let trimmed = cleaned.trim_end_matches([' ', '\t']);
    let Some((before, last)) = trimmed.rsplit_once(char::is_whitespace) else {
        return format!("{trimmed}  {completion}");
    };
    let id = last.strip_prefix('^').unwrap_or("");
    if !capture_language::is_block_id(id) {
        return format!("{trimmed}  {completion}");
    }
    format!(
        "{}  {completion} ^{id}",
        before.trim_end_matches([' ', '\t'])
    )
}

fn retire_embedded_links(
    line: &str,
    targets: &BTreeSet<TaskKey>,
    vault: &impl CloseVault,
    day_path: &Path,
) -> String {
    let tokens = wikilink_tokens(line);
    let mut edits = Vec::new();
    for token in tokens.iter().filter(|token| token.embedded) {
        let LinkResolution::Found(path) =
            vault.resolve_target(day_path, &token.path_part)
        else {
            continue;
        };
        if !targets.contains(&(path, token.block_id.clone())) {
            continue;
        }
        let prefix = pomodoro_marker_prefix(line, token.start);
        edits.push((prefix.start, token.end, format!("~~{}~~", token.token)));
    }
    apply_edits(line, edits)
}

//! Embedded task-tree close shared by `=x` and whole-item completion.
//!
//! This is the extracted `ClosePlanner::apply_embedded_tree` traversal from
//! `capture_pomodoro_close::linked_tasks`, parameterized by a root policy:
//! `CloseLink` preserves today's `=x` behavior exactly, while `Explicit`
//! additionally closes a Blocked root and leaves recurring lines open with
//! a reason instead of closing them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::super::{
    capture::{line_spans, LineSpan},
    capture_language,
    capture_pomodoro_close::{
        lookup_task, target_from_token, wikilink_tokens, BlockLinkTarget,
        CloseVault,
    },
    capture_task_toggle::set_task_line_status,
    note_tasks::{self, NoteTask, NoteTaskSettings},
    task_dependencies::is_dependency_line,
};

use super::is_recurring_task_line;

/// Depth and target caps inherited from the close traversal.
pub(crate) const MAX_TREE_DEPTH: usize = 25;
pub(crate) const MAX_TREE_TARGETS: usize = 250;

/// Root policy for [`complete_task_tree`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RootPolicy {
    /// Today's `=x` behavior: only ` `/`*`/`/` roots close.
    CloseLink,
    /// Whole-item completion: a Blocked root closes too.
    Explicit,
}

/// Why a descendant was left open instead of closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LeftOpenReason {
    Blocked,
    Recurring,
    UnknownStatus,
    Cap,
}

impl LeftOpenReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::Recurring => "recurring",
            Self::UnknownStatus => "unknown_status",
            Self::Cap => "cap",
        }
    }
}

/// One closed task line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClosedTask {
    pub(crate) absolute_path: PathBuf,
    pub(crate) relative_path: String,
    pub(crate) block_id: String,
    /// 1-based line number.
    pub(crate) line: usize,
    pub(crate) text: String,
    pub(crate) previous_status_symbol: char,
    pub(crate) previous_status_name: String,
    pub(crate) status_symbol: char,
    pub(crate) status_name: String,
}

/// One descendant left open, with the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LeftOpenTask {
    pub(crate) absolute_path: PathBuf,
    pub(crate) relative_path: String,
    pub(crate) block_id: String,
    /// 1-based line number.
    pub(crate) line: usize,
    pub(crate) text: String,
    pub(crate) status_symbol: char,
    pub(crate) status_name: String,
    pub(crate) reason: LeftOpenReason,
}

/// Structured result of [`complete_task_tree`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct CompleteTreeOutcome {
    /// Post-images for changed files only.
    pub(crate) changed_files: BTreeMap<PathBuf, String>,
    /// The closed root, when the root itself closed.
    pub(crate) root: Option<ClosedTask>,
    /// Closed subtasks in close order (children before parents).
    pub(crate) closed_subtasks: Vec<ClosedTask>,
    pub(crate) left_open: Vec<LeftOpenTask>,
    pub(crate) warnings: Vec<String>,
}

/// Whether `symbol` may be closed under `policy`. `CloseLink` matches the
/// long-standing close; `Explicit` additionally admits a Blocked root.
pub(crate) fn close_policy_allows(policy: RootPolicy, symbol: char) -> bool {
    match symbol {
        ' ' | '*' | '/' => true,
        '?' => policy == RootPolicy::Explicit,
        _ => false,
    }
}

/// Whether traversal continues through `symbol` under `policy`. Done lines
/// are traversed (their open children still close) but never re-closed.
pub(crate) fn close_traversal_gate(policy: RootPolicy, symbol: char) -> bool {
    match symbol {
        ' ' | '*' | '/' | 'x' => true,
        'X' | '?' => policy == RootPolicy::Explicit,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Line helpers extracted from `capture_pomodoro_close::linked_tasks`.
// The close planner reuses these, so both paths share one definition.
// ---------------------------------------------------------------------------

pub(crate) fn embedded_children(
    contents: &str,
    task: &NoteTask,
) -> Vec<BlockLinkTarget> {
    let spans = line_spans(contents);
    let mut children = Vec::new();
    for (index, span) in spans.iter().enumerate().skip(task.line_index + 1) {
        if span.end > task.block_end {
            break;
        }
        // Closing a dependent never closes its prerequisites: Depends-On
        // lines are not embedded-tree edges
        // (`docs/task-dependencies.md` §5).
        if is_dependency_line(span.text) {
            continue;
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

pub(crate) fn line_text_at(contents: &str, line_index: usize) -> Option<&str> {
    line_spans(contents).get(line_index).map(|line| line.text)
}

pub(crate) fn replace_line(
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

pub(crate) fn normalize_task_metadata_spacing(line: &str) -> String {
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

pub(crate) fn task_has_global_filter(
    line: &str,
    settings: &NoteTaskSettings,
) -> bool {
    !settings.global_filter.is_empty()
        && line
            .split_whitespace()
            .any(|token| token == settings.global_filter)
}

pub(crate) fn remove_completion_fields(line: &str) -> String {
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

pub(crate) fn add_or_replace_completion_field(
    line: &str,
    date: &str,
) -> String {
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

// ---------------------------------------------------------------------------
// Tree traversal.
// ---------------------------------------------------------------------------

type TaskKey = (PathBuf, String);

struct TreeWork<'a, V> {
    vault: &'a V,
    settings: NoteTaskSettings,
    all_task_settings: NoteTaskSettings,
    date: String,
    policy: RootPolicy,
    below_line: usize,
    staged: BTreeMap<PathBuf, String>,
    originals: BTreeMap<PathBuf, String>,
    visited: BTreeSet<TaskKey>,
    visited_count: usize,
    warnings: Vec<String>,
    root: Option<ClosedTask>,
    closed: Vec<ClosedTask>,
    left_open: Vec<LeftOpenTask>,
}

impl<'a, V: CloseVault> TreeWork<'a, V> {
    fn relative_target(&self, path: &Path) -> String {
        path.strip_prefix(self.vault.bob_dir())
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    fn read_file(&mut self, path: &Path) -> Result<Option<String>, String> {
        if let Some(contents) = self.staged.get(path) {
            return Ok(Some(contents.clone()));
        }
        let contents = self.vault.read_latest(path).map_err(|error| error)?;
        if let Some(contents) = contents {
            self.originals.insert(path.to_path_buf(), contents.clone());
            self.staged.insert(path.to_path_buf(), contents.clone());
            Ok(Some(contents))
        } else {
            Ok(None)
        }
    }

    fn warn(&mut self, message: String) {
        if !self.warnings.contains(&message) {
            self.warnings.push(message);
        }
    }

    fn record_left_open(
        &mut self,
        key: &TaskKey,
        task: &NoteTask,
        reason: LeftOpenReason,
    ) {
        self.left_open.push(LeftOpenTask {
            absolute_path: key.0.clone(),
            relative_path: self.relative_target(&key.0),
            block_id: key.1.clone(),
            line: task.line_index + 1,
            text: task.description.clone(),
            status_symbol: task.status_symbol,
            status_name: task.status_name.clone(),
            reason,
        });
    }

    fn close_node(
        &mut self,
        from_path: &Path,
        path: &Path,
        block_id: &str,
        wikilink: &str,
        depth: usize,
        is_root: bool,
    ) -> Result<(), String> {
        if depth > MAX_TREE_DEPTH {
            self.warn(format!(
                "embedded task recursion exceeded 25 levels below line {}",
                self.below_line
            ));
            if let Some(contents) = self.read_file(path)?
                && let Ok(task) =
                    lookup_task(&contents, &self.all_task_settings, block_id)
            {
                self.record_left_open(
                    &(path.to_path_buf(), block_id.to_string()),
                    &task,
                    LeftOpenReason::Cap,
                );
            }
            return Ok(());
        }
        let key = (path.to_path_buf(), block_id.to_string());
        if self.visited.contains(&key) {
            return Ok(());
        }
        if self.visited_count >= MAX_TREE_TARGETS {
            self.warn(
                "embedded task recursion exceeded 250 targets".to_string(),
            );
            if let Some(contents) = self.read_file(path)?
                && let Ok(task) =
                    lookup_task(&contents, &self.all_task_settings, block_id)
            {
                self.record_left_open(&key, &task, LeftOpenReason::Cap);
            }
            return Ok(());
        }
        self.visited.insert(key.clone());
        self.visited_count += 1;

        let Some(contents) = self.read_file(path)? else {
            let message = format!(
                "{wikilink} resolves to {}, which cannot be read; the target was left unchanged",
                self.relative_target(path)
            );
            self.warn(message);
            return Ok(());
        };
        let task =
            match lookup_task(&contents, &self.all_task_settings, block_id) {
                Ok(task) => task,
                Err(reason) => {
                    if is_root {
                        return Err(format!(
                            "{wikilink} in {} {reason}",
                            self.relative_target(path)
                        ));
                    }
                    self.warn(format!(
                    "{wikilink} in {} {reason}; the target was left unchanged",
                    self.relative_target(path)
                ));
                    return Ok(());
                }
            };
        if is_root && self.below_line == 0 {
            self.below_line = task.line_index + 1;
        }
        let symbol = task.status_symbol;
        if symbol == 'x'
            || (self.policy == RootPolicy::Explicit && symbol == 'X')
        {
            // Already done: close the open children, if any, but record
            // nothing for this line.
            let descendants = embedded_children(&contents, &task);
            for descendant in descendants {
                self.close_descendant(path, &descendant, depth + 1)?;
            }
            return Ok(());
        }
        if self.policy == RootPolicy::Explicit
            && let Some(line) = line_text_at(&contents, task.line_index)
            && is_recurring_task_line(line)
        {
            self.record_left_open(&key, &task, LeftOpenReason::Recurring);
            return Ok(());
        }
        let allowed = if is_root {
            close_policy_allows(self.policy, symbol)
        } else {
            close_policy_allows(RootPolicy::CloseLink, symbol)
        };
        if !allowed {
            let traverses = close_traversal_gate(self.policy, symbol);
            if symbol == '?' {
                // Blocked lines never close, except for an `Explicit`
                // root, which took the `allowed` path above.
                self.record_left_open(&key, &task, LeftOpenReason::Blocked);
            } else if !traverses {
                self.record_left_open(
                    &key,
                    &task,
                    LeftOpenReason::UnknownStatus,
                );
            }
            if traverses {
                // An `Explicit` Blocked descendant or done `X` line:
                // close the open children without closing this line.
                let descendants = embedded_children(&contents, &task);
                for descendant in descendants {
                    self.close_descendant(path, &descendant, depth + 1)?;
                }
            }
            return Ok(());
        }

        let descendants = embedded_children(&contents, &task);
        for descendant in descendants {
            self.close_descendant(path, &descendant, depth + 1)?;
        }

        let Some(contents) = self.read_file(path)? else {
            return Ok(());
        };
        let Ok(current) =
            lookup_task(&contents, &self.all_task_settings, block_id)
        else {
            return Ok(());
        };
        let allowed_now = if is_root {
            close_policy_allows(self.policy, current.status_symbol)
        } else {
            close_policy_allows(RootPolicy::CloseLink, current.status_symbol)
        };
        if !allowed_now {
            return Ok(());
        }
        let Some(line) = self.staged.get(path).and_then(|contents| {
            line_text_at(contents, current.line_index).map(str::to_string)
        }) else {
            return Ok(());
        };
        if self.policy == RootPolicy::Explicit && is_recurring_task_line(&line)
        {
            self.record_left_open(&key, &current, LeftOpenReason::Recurring);
            return Ok(());
        }
        let Some(mut updated) = set_task_line_status(&line, 'x') else {
            return Ok(());
        };
        if task_has_global_filter(&line, &self.settings) {
            updated = add_or_replace_completion_field(&updated, &self.date);
        }
        let Some(next) = replace_line(&contents, current.line_index, &updated)
        else {
            return Ok(());
        };
        if next == contents {
            return Ok(());
        }
        self.staged.insert(path.to_path_buf(), next);
        let status_name = self
            .read_file(path)?
            .and_then(|contents| {
                lookup_task(&contents, &self.all_task_settings, block_id).ok()
            })
            .map(|task| task.status_name.clone())
            .unwrap_or_else(|| "Done".to_string());
        let transition = ClosedTask {
            absolute_path: path.to_path_buf(),
            relative_path: self.relative_target(path),
            block_id: block_id.to_string(),
            line: current.line_index + 1,
            text: current.description.clone(),
            previous_status_symbol: current.status_symbol,
            previous_status_name: current.status_name.clone(),
            status_symbol: 'x',
            status_name,
        };
        if is_root {
            self.root = Some(transition);
        } else {
            self.closed.push(transition);
        }
        let _ = from_path;
        Ok(())
    }

    fn close_descendant(
        &mut self,
        from_path: &Path,
        descendant: &BlockLinkTarget,
        depth: usize,
    ) -> Result<(), String> {
        let path = match self
            .vault
            .resolve_target(from_path, &descendant.path_part)
        {
            super::super::vault_links::LinkResolution::Found(path) => path,
            super::super::vault_links::LinkResolution::Ambiguous => {
                self.warn(format!(
                    "{} has an ambiguous note basename; the target was left unchanged",
                    descendant.wikilink
                ));
                return Ok(());
            }
            super::super::vault_links::LinkResolution::Missing => {
                self.warn(format!(
                    "{} does not resolve to a vault note; the target was left unchanged",
                    descendant.wikilink
                ));
                return Ok(());
            }
        };
        self.close_node(
            from_path,
            &path,
            &descendant.block_id,
            &descendant.wikilink,
            depth,
            false,
        )
    }
}

/// Close the embedded task tree rooted at (`root_path`, `block_id`).
///
/// The root closes under `policy` (`CloseLink` keeps today's `=x` rules;
/// `Explicit` also admits a Blocked root). Descendants close under the
/// existing close rules: Depends-On lines are skipped, the depth cap is 25,
/// the target cap is 250, and only ` `/`*`/`/` descendants close. Under
/// `Explicit`, recurring lines are left open with a reason instead. The
/// root line matches what `=x` writes, including the `  [completion::
/// YYYY-MM-DD]` field before a trailing `^id` when the line carries the
/// global-filter tag.
pub(crate) fn complete_task_tree<V: CloseVault>(
    vault: &V,
    root_path: &Path,
    block_id: &str,
    completion_date: &str,
    policy: RootPolicy,
) -> Result<CompleteTreeOutcome, String> {
    let settings = note_tasks::read_settings(vault.bob_dir());
    let mut all_task_settings = settings.clone();
    all_task_settings.global_filter.clear();
    let mut work = TreeWork {
        vault,
        settings,
        all_task_settings,
        date: completion_date.to_string(),
        policy,
        below_line: 0,
        staged: BTreeMap::new(),
        originals: BTreeMap::new(),
        visited: BTreeSet::new(),
        visited_count: 0,
        warnings: Vec::new(),
        root: None,
        closed: Vec::new(),
        left_open: Vec::new(),
    };
    let wikilink = format!("#^{block_id}");
    work.close_node(root_path, root_path, block_id, &wikilink, 0, true)?;
    let changed_files = work
        .staged
        .iter()
        .filter(|(path, contents)| work.originals.get(*path) != Some(*contents))
        .map(|(path, contents)| (path.clone(), contents.clone()))
        .collect();
    Ok(CompleteTreeOutcome {
        changed_files,
        root: work.root,
        closed_subtasks: work.closed,
        left_open: work.left_open,
        warnings: work.warnings,
    })
}

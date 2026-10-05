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
    note_tasks::{self, NoteTask, NoteTaskSettings, TaskStatusType},
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

/// One `=x` embedded-tree root: the ledger link plus the file it was
/// found in. The close replays [`EmbeddedTreeVisit`]s in order to rebuild
/// its task rows, so the engine records link identity (not snapshots) per
/// visit and the close re-reads pre-merge staged text exactly once.
pub(crate) struct EmbeddedTreeRoot<'a> {
    pub(crate) from_path: &'a Path,
    pub(crate) target: &'a BlockLinkTarget,
}

/// What the traversal did at one visited node, in visit order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EmbeddedVisitOutcome {
    /// The node resolved to a task. The close registers its row; `closed`
    /// selects the post-merge row refresh.
    Resolved { closed: bool },
    /// Resolution, read, or lookup failed. The close registers the link,
    /// which reproduces the row and its warning.
    Unresolved,
    /// Depth cap: the close warns only and registers no row, matching the
    /// long-standing `=x` behavior.
    DepthCapped { warning: String },
    /// Target cap: the close registers the row, then warns.
    TargetCapped { warning: String },
}

/// One node visited by the embedded-tree traversal, in visit order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EmbeddedTreeVisit {
    /// File the link was found in.
    pub(crate) from_path: PathBuf,
    pub(crate) path_part: String,
    pub(crate) block_id: String,
    /// Link text as written (`[[sase#^id]]` / `![[sase#^id]]` form).
    pub(crate) wikilink: String,
    /// Ledger line of the root that reached this node.
    pub(crate) ledger_line: usize,
    pub(crate) is_root: bool,
    pub(crate) outcome: EmbeddedVisitOutcome,
}

/// Structured result of [`complete_embedded_trees`]: one shared traversal
/// over every `=x` embedded root, replayed by the close planner.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct EmbeddedTreesOutcome {
    /// Post-images for changed files only.
    pub(crate) changed_files: BTreeMap<PathBuf, String>,
    /// Every visited node, in visit order.
    pub(crate) visits: Vec<EmbeddedTreeVisit>,
    /// Warnings in traversal order. The close regenerates these inline
    /// while replaying visits, so it ignores this list.
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
    /// When set, depth-cap warnings use this line instead of `below_line`.
    /// The `=x` close sets it per root to the root's ledger line so its
    /// long-standing warning text stays byte-identical.
    warn_line_override: Option<usize>,
    /// When set, root lookup failures warn instead of erroring, so the
    /// `=x` close can replay them as unresolved rows.
    lenient_roots: bool,
    staged: BTreeMap<PathBuf, String>,
    originals: BTreeMap<PathBuf, String>,
    visited: BTreeSet<TaskKey>,
    visited_count: usize,
    warnings: Vec<String>,
    root: Option<ClosedTask>,
    closed: Vec<ClosedTask>,
    left_open: Vec<LeftOpenTask>,
    visits: Vec<EmbeddedTreeVisit>,
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
        let contents = self.vault.read_latest(path)?;
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

    #[allow(clippy::too_many_arguments)]
    fn push_visit(
        &mut self,
        from_path: &Path,
        path_part: &str,
        block_id: &str,
        wikilink: &str,
        ledger_line: usize,
        is_root: bool,
        outcome: EmbeddedVisitOutcome,
    ) {
        self.visits.push(EmbeddedTreeVisit {
            from_path: from_path.to_path_buf(),
            path_part: path_part.to_string(),
            block_id: block_id.to_string(),
            wikilink: wikilink.to_string(),
            ledger_line,
            is_root,
            outcome,
        });
    }

    fn warn_label(&self) -> usize {
        self.warn_line_override.unwrap_or(self.below_line)
    }

    #[allow(clippy::too_many_arguments)]
    fn close_node(
        &mut self,
        from_path: &Path,
        path: &Path,
        path_part: &str,
        block_id: &str,
        wikilink: &str,
        depth: usize,
        is_root: bool,
        ledger_line: usize,
    ) -> Result<(), String> {
        if depth > MAX_TREE_DEPTH {
            let warning = format!(
                "embedded task recursion exceeded 25 levels below line {}",
                self.warn_label()
            );
            self.warn(warning.clone());
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
            self.push_visit(
                from_path,
                path_part,
                block_id,
                wikilink,
                ledger_line,
                is_root,
                EmbeddedVisitOutcome::DepthCapped { warning },
            );
            return Ok(());
        }
        let key = (path.to_path_buf(), block_id.to_string());
        if self.visited.contains(&key) {
            return Ok(());
        }
        if self.visited_count >= MAX_TREE_TARGETS {
            let warning =
                "embedded task recursion exceeded 250 targets".to_string();
            self.warn(warning.clone());
            if let Some(contents) = self.read_file(path)?
                && let Ok(task) =
                    lookup_task(&contents, &self.all_task_settings, block_id)
            {
                self.record_left_open(&key, &task, LeftOpenReason::Cap);
            }
            self.push_visit(
                from_path,
                path_part,
                block_id,
                wikilink,
                ledger_line,
                is_root,
                EmbeddedVisitOutcome::TargetCapped { warning },
            );
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
            self.push_visit(
                from_path,
                path_part,
                block_id,
                wikilink,
                ledger_line,
                is_root,
                EmbeddedVisitOutcome::Unresolved,
            );
            return Ok(());
        };
        let task =
            match lookup_task(&contents, &self.all_task_settings, block_id) {
                Ok(task) => task,
                Err(reason) => {
                    if is_root && !self.lenient_roots {
                        return Err(format!(
                            "{wikilink} in {} {reason}",
                            self.relative_target(path)
                        ));
                    }
                    self.warn(format!(
                    "{wikilink} in {} {reason}; the target was left unchanged",
                    self.relative_target(path)
                ));
                    self.push_visit(
                        from_path,
                        path_part,
                        block_id,
                        wikilink,
                        ledger_line,
                        is_root,
                        EmbeddedVisitOutcome::Unresolved,
                    );
                    return Ok(());
                }
            };
        if is_root && self.below_line == 0 {
            self.below_line = task.line_index + 1;
        }
        let symbol = task.status_symbol;
        if matches!(
            task.status_type,
            TaskStatusType::Done | TaskStatusType::Cancelled
        ) {
            // Done and Canceled lines are neither closed nor reported.
            // A done `x` line is still traversed so its open children
            // close, exactly as `=x` always did.
            if symbol == 'x' {
                let descendants = embedded_children(&contents, &task);
                for descendant in descendants {
                    self.close_descendant(
                        path,
                        &descendant,
                        depth + 1,
                        ledger_line,
                    )?;
                }
            }
            self.push_visit(
                from_path,
                path_part,
                block_id,
                wikilink,
                ledger_line,
                is_root,
                EmbeddedVisitOutcome::Resolved { closed: false },
            );
            return Ok(());
        }
        if self.policy == RootPolicy::Explicit
            && let Some(line) = line_text_at(&contents, task.line_index)
            && is_recurring_task_line(line)
        {
            self.record_left_open(&key, &task, LeftOpenReason::Recurring);
            self.push_visit(
                from_path,
                path_part,
                block_id,
                wikilink,
                ledger_line,
                is_root,
                EmbeddedVisitOutcome::Resolved { closed: false },
            );
            return Ok(());
        }
        let allowed = if is_root {
            close_policy_allows(self.policy, symbol)
        } else {
            close_policy_allows(RootPolicy::CloseLink, symbol)
        };
        if !allowed {
            // Descendants always use the `CloseLink` gate: only the root
            // may differ, so traversal never descends through a Blocked
            // `[?]` or `X` descendant.
            let traverses = close_traversal_gate(
                if is_root {
                    self.policy
                } else {
                    RootPolicy::CloseLink
                },
                symbol,
            );
            if symbol == '?' {
                // Blocked lines never close, except for an `Explicit`
                // root, which took the `allowed` path above. A Blocked
                // descendant's own embedded children are not visited.
                self.record_left_open(&key, &task, LeftOpenReason::Blocked);
            } else if !traverses {
                self.record_left_open(
                    &key,
                    &task,
                    LeftOpenReason::UnknownStatus,
                );
            }
            if traverses {
                // A done `x` line that fell through (or an `Explicit`
                // root admitted above): close the open children without
                // closing this line.
                let descendants = embedded_children(&contents, &task);
                for descendant in descendants {
                    self.close_descendant(
                        path,
                        &descendant,
                        depth + 1,
                        ledger_line,
                    )?;
                }
            }
            self.push_visit(
                from_path,
                path_part,
                block_id,
                wikilink,
                ledger_line,
                is_root,
                EmbeddedVisitOutcome::Resolved { closed: false },
            );
            return Ok(());
        }

        let descendants = embedded_children(&contents, &task);
        for descendant in descendants {
            self.close_descendant(path, &descendant, depth + 1, ledger_line)?;
        }

        let closed = self.finish_close_node(&key, path, block_id, is_root)?;
        self.push_visit(
            from_path,
            path_part,
            block_id,
            wikilink,
            ledger_line,
            is_root,
            EmbeddedVisitOutcome::Resolved { closed },
        );
        Ok(())
    }

    /// Re-read the node after its descendants closed and close it when it
    /// is still closable. Returns whether the staged text changed.
    fn finish_close_node(
        &mut self,
        key: &TaskKey,
        path: &Path,
        block_id: &str,
        is_root: bool,
    ) -> Result<bool, String> {
        let Some(contents) = self.read_file(path)? else {
            return Ok(false);
        };
        let Ok(current) =
            lookup_task(&contents, &self.all_task_settings, block_id)
        else {
            return Ok(false);
        };
        let allowed_now = if is_root {
            close_policy_allows(self.policy, current.status_symbol)
        } else {
            close_policy_allows(RootPolicy::CloseLink, current.status_symbol)
        };
        if !allowed_now {
            return Ok(false);
        }
        let Some(line) = self.staged.get(path).and_then(|contents| {
            line_text_at(contents, current.line_index).map(str::to_string)
        }) else {
            return Ok(false);
        };
        if self.policy == RootPolicy::Explicit && is_recurring_task_line(&line)
        {
            self.record_left_open(key, &current, LeftOpenReason::Recurring);
            return Ok(false);
        }
        let Some(mut updated) = set_task_line_status(&line, 'x') else {
            return Ok(false);
        };
        if task_has_global_filter(&line, &self.settings) {
            updated = add_or_replace_completion_field(&updated, &self.date);
        }
        let Some(next) = replace_line(&contents, current.line_index, &updated)
        else {
            return Ok(false);
        };
        if next == contents {
            return Ok(false);
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
        Ok(true)
    }

    fn close_descendant(
        &mut self,
        from_path: &Path,
        descendant: &BlockLinkTarget,
        depth: usize,
        ledger_line: usize,
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
                self.push_visit(
                    from_path,
                    &descendant.path_part,
                    &descendant.block_id,
                    &descendant.wikilink,
                    ledger_line,
                    false,
                    EmbeddedVisitOutcome::Unresolved,
                );
                return Ok(());
            }
            super::super::vault_links::LinkResolution::Missing => {
                self.warn(format!(
                    "{} does not resolve to a vault note; the target was left unchanged",
                    descendant.wikilink
                ));
                self.push_visit(
                    from_path,
                    &descendant.path_part,
                    &descendant.block_id,
                    &descendant.wikilink,
                    ledger_line,
                    false,
                    EmbeddedVisitOutcome::Unresolved,
                );
                return Ok(());
            }
        };
        self.close_node(
            from_path,
            &path,
            &descendant.path_part,
            &descendant.block_id,
            &descendant.wikilink,
            depth,
            false,
            ledger_line,
        )
    }
}

/// Close the embedded task tree rooted at (`root_path`, `block_id`).
///
/// The root closes under `policy` (`CloseLink` keeps today's `=x` rules;
/// `Explicit` also admits a Blocked root). Only the root may differ from
/// the close rules: descendants always use the `CloseLink` traversal gate,
/// so a Blocked `[?]` or `X` descendant (and everything below it) stays
/// open. Depends-On lines are skipped, the depth cap is 25, the target cap
/// is 250, and only ` `/`*`/`/` descendants close. Under `Explicit`,
/// recurring lines are left open with a reason instead. Done and Canceled
/// descendants (resolved through the Tasks status settings) are neither
/// closed nor reported. The root line matches what `=x` writes, including
/// the `  [completion:: YYYY-MM-DD]` field before a trailing `^id` when
/// the line carries the global-filter tag.
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
        warn_line_override: None,
        lenient_roots: false,
        staged: BTreeMap::new(),
        originals: BTreeMap::new(),
        visited: BTreeSet::new(),
        visited_count: 0,
        warnings: Vec::new(),
        root: None,
        closed: Vec::new(),
        left_open: Vec::new(),
        visits: Vec::new(),
    };
    let wikilink = format!("#^{block_id}");
    work.close_node(root_path, root_path, "", block_id, &wikilink, 0, true, 0)?;
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

/// Close every `=x` embedded tree in one shared traversal.
///
/// This is the single traversal behind `=x`: the depth cap, the target
/// cap, the Depends-On skip, and the gate all live here, and the close
/// planner replays [`EmbeddedTreeVisit`]s instead of recursing on its own.
/// Each root's depth-cap warnings name its own ledger line, and root link
/// text is preserved verbatim so replayed warnings stay byte-identical.
pub(crate) fn complete_embedded_trees<V: CloseVault>(
    vault: &V,
    roots: &[EmbeddedTreeRoot<'_>],
    completion_date: &str,
) -> Result<EmbeddedTreesOutcome, String> {
    let settings = note_tasks::read_settings(vault.bob_dir());
    let mut all_task_settings = settings.clone();
    all_task_settings.global_filter.clear();
    let mut work = TreeWork {
        vault,
        settings,
        all_task_settings,
        date: completion_date.to_string(),
        policy: RootPolicy::CloseLink,
        below_line: 0,
        warn_line_override: None,
        lenient_roots: true,
        staged: BTreeMap::new(),
        originals: BTreeMap::new(),
        visited: BTreeSet::new(),
        visited_count: 0,
        warnings: Vec::new(),
        root: None,
        closed: Vec::new(),
        left_open: Vec::new(),
        visits: Vec::new(),
    };
    for root in roots {
        let path = match vault
            .resolve_target(root.from_path, &root.target.path_part)
        {
            super::super::vault_links::LinkResolution::Found(path) => path,
            super::super::vault_links::LinkResolution::Ambiguous => {
                work.warn(format!(
                    "{} has an ambiguous note basename; the target was left unchanged",
                    root.target.wikilink
                ));
                work.push_visit(
                    root.from_path,
                    &root.target.path_part,
                    &root.target.block_id,
                    &root.target.wikilink,
                    root.target.line,
                    true,
                    EmbeddedVisitOutcome::Unresolved,
                );
                continue;
            }
            super::super::vault_links::LinkResolution::Missing => {
                work.warn(format!(
                    "{} does not resolve to a vault note; the target was left unchanged",
                    root.target.wikilink
                ));
                work.push_visit(
                    root.from_path,
                    &root.target.path_part,
                    &root.target.block_id,
                    &root.target.wikilink,
                    root.target.line,
                    true,
                    EmbeddedVisitOutcome::Unresolved,
                );
                continue;
            }
        };
        work.warn_line_override = Some(root.target.line);
        work.close_node(
            root.from_path,
            &path,
            &root.target.path_part,
            &root.target.block_id,
            &root.target.wikilink,
            0,
            true,
            root.target.line,
        )?;
    }
    let changed_files = work
        .staged
        .iter()
        .filter(|(path, contents)| work.originals.get(*path) != Some(*contents))
        .map(|(path, contents)| (path.clone(), contents.clone()))
        .collect();
    Ok(EmbeddedTreesOutcome {
        changed_files,
        visits: work.visits,
        warnings: work.warnings,
    })
}

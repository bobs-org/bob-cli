//! Resolved agenda payload for `bob capture-pomodoros --tasks`.
//!
//! One memoized pass over the vault: each distinct note is read and
//! scanned once per filter mode, then every open entry's Task Link lineup
//! is resolved through that cache. Link recognition reuses the close
//! planner's numbered walk
//! ([`number_task_links_in_range`](super::capture_pomodoro_close::number_task_links_in_range)),
//! so the `=x` numbers below are exactly the numbers `bob capture -- =x`
//! will use, and the other open entries carry the `=` lineup numbers from
//! [`list_queued_links`](super::capture_pomodoro_start::list_queued_links).

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    ops::Range,
    path::{Path, PathBuf},
};

use chrono::NaiveDate;
use serde::Serialize;

use super::{
    capture::{
        block_depths, line_spans, list_item_body,
        nearest_shallower_list_item_parent, parse_managed_task_log_marker,
        LineSpan, ManagedTaskLogKind,
    },
    capture_pomodoro_close::{
        bare_embedded_link, bare_plain_link, close_task_text,
        number_task_links_in_range, strikethrough_inner_spans,
        strip_pomodoro_markers, sub_bullet_range, wikilink_tokens,
        NumberedTaskLink, TaskLinkMarker,
    },
    capture_pomodoro_start::list_queued_links,
    capture_pomodoros::bounded_warning,
    capture_tasks::status_type_label,
    collect_done::trailing_block_id_in_line,
    env as bob_env, markdown, note_tasks, pomodoro,
    vault_links::{LinkResolution, VaultLinkResolver},
};

/// Lines kept per task block after truncation.
pub(crate) const MAX_TASK_BLOCK_LINES: usize = 150;

/// Entry role in the agenda, from the ledger facts only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgendaRole {
    Current,
    Next,
    Later,
    Open,
    Completed,
}

/// How one Task Link resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgendaResolution {
    Resolved,
    MissingNote,
    AmbiguousNote,
    MissingBlock,
    DuplicateBlock,
    NotATask,
    Unreadable,
}

/// `work` or `schedule` log tag on a display line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum AgendaLog {
    Work,
    Schedule,
}

impl AgendaLog {
    fn from_kind(kind: ManagedTaskLogKind) -> Self {
        match kind {
            ManagedTaskLogKind::Work => Self::Work,
            ManagedTaskLogKind::Schedule => Self::Schedule,
        }
    }
}

/// One display line inside a task block, a ledger note, or an entry note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct AgendaLine {
    pub(crate) text: String,
    pub(crate) depth: usize,
    pub(crate) kind: AgendaLineKind,
    pub(crate) status_symbol: Option<char>,
    pub(crate) log: Option<AgendaLog>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgendaLineKind {
    Bullet,
    Task,
    Text,
    Code,
    LogMarker,
}

/// One resolved Task Link line on an open entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct AgendaItem {
    pub(crate) index: Option<u32>,
    pub(crate) ledger_line: usize,
    pub(crate) ledger_depth: usize,
    pub(crate) marker: String,
    pub(crate) block_link: String,
    pub(crate) resolution: AgendaResolution,
    pub(crate) relative_target: Option<String>,
    pub(crate) line: Option<usize>,
    pub(crate) block_id: String,
    pub(crate) text: Option<String>,
    pub(crate) status_symbol: Option<char>,
    pub(crate) status_name: Option<String>,
    pub(crate) status_type: Option<String>,
    pub(crate) lines: Vec<AgendaLine>,
    pub(crate) lines_truncated: usize,
    pub(crate) ledger_notes: Vec<AgendaLine>,
    pub(crate) warning: Option<String>,
}

/// Per-entry agenda additions for `--tasks`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct AgendaEntry {
    pub(crate) role: AgendaRole,
    pub(crate) starts_at: Option<String>,
    pub(crate) ends_at: Option<String>,
    pub(crate) retired_link_count: usize,
    pub(crate) notes: Vec<AgendaLine>,
    pub(crate) items: Vec<AgendaItem>,
}

/// Top-level agenda additions for `--tasks`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct CompletedSummary {
    pub(crate) count: usize,
    pub(crate) minutes: u64,
}

/// Shared ledger context for resolving one entry's items: the daily
/// note's precomputed spans, fences, range, and item lines.
struct ItemContext<'a, 'c> {
    day_contents_lines: &'a [&'c str],
    day_spans: &'a [LineSpan<'c>],
    day_fenced: &'a BTreeSet<usize>,
    entry_range: &'a Range<usize>,
    item_lines: &'a BTreeSet<usize>,
    day_path: &'a Path,
}

/// One link line to resolve, with its operator number already assigned.
#[derive(Debug, Clone, Copy)]
struct ItemLink<'a> {
    ledger_line: usize,
    ledger_depth: usize,
    marker: TaskLinkMarker,
    block_link: &'a str,
    path_part: &'a str,
    block_id: &'a str,
    index: Option<u32>,
}

/// Memoized note reader for one agenda run. Each distinct note is read
/// from disk and scanned once per filter mode; the already-read daily
/// note is seeded without a filesystem read.
pub(crate) struct AgendaNotes<'a> {
    bob_dir: &'a Path,
    day_path: PathBuf,
    resolver: VaultLinkResolver,
    settings: note_tasks::NoteTaskSettings,
    cleared: note_tasks::NoteTaskSettings,
    cache: HashMap<(PathBuf, bool), Option<(String, note_tasks::NoteTaskScan)>>,
    #[cfg(test)]
    reads: usize,
}

impl<'a> AgendaNotes<'a> {
    pub(crate) fn new(
        bob_dir: &'a Path,
        day_path: &Path,
        day_contents: &str,
    ) -> Self {
        let settings = note_tasks::read_settings(bob_dir);
        let mut cleared = settings.clone();
        cleared.global_filter.clear();
        let mut notes = Self {
            bob_dir,
            day_path: day_path.to_path_buf(),
            resolver: VaultLinkResolver::new(bob_dir),
            settings,
            cleared,
            cache: HashMap::new(),
            #[cfg(test)]
            reads: 0,
        };
        for filter_cleared in [false, true] {
            let scan = note_tasks::scan(
                day_contents,
                notes.settings_for(filter_cleared),
            );
            notes.cache.insert(
                (notes.day_path.clone(), filter_cleared),
                Some((day_contents.to_string(), scan)),
            );
        }
        notes
    }

    fn settings_for(
        &self,
        filter_cleared: bool,
    ) -> &note_tasks::NoteTaskSettings {
        if filter_cleared {
            &self.cleared
        } else {
            &self.settings
        }
    }

    fn global_filter_for(&self, filter_cleared: bool) -> &str {
        if filter_cleared {
            ""
        } else {
            &self.settings.global_filter
        }
    }

    /// Vault-relative display path for a resolved absolute path.
    pub(crate) fn relative_target(&self, path: &Path) -> String {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.bob_dir.join(path)
        };
        absolute
            .strip_prefix(self.bob_dir)
            .unwrap_or(&absolute)
            .to_string_lossy()
            .to_string()
    }

    /// Read (once) and scan (once per filter mode) the note behind one
    /// link target. `None` when the note cannot be read.
    fn note_for(
        &mut self,
        path: &Path,
        filter_cleared: bool,
    ) -> Option<(String, note_tasks::NoteTaskScan)> {
        let key = (path.to_path_buf(), filter_cleared);
        if let Some(cached) = self.cache.get(&key) {
            return cached.clone();
        }
        #[cfg(test)]
        {
            self.reads += 1;
        }
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(_) => {
                self.cache.insert(key, None);
                return None;
            }
        };
        let scan =
            note_tasks::scan(&contents, self.settings_for(filter_cleared));
        self.cache
            .insert(key, Some((contents.clone(), scan.clone())));
        Some((contents, scan))
    }

    #[cfg(test)]
    pub(crate) fn test_reads(&self) -> usize {
        self.reads
    }

    /// Resolve one link line into its agenda item payload.
    fn resolve_item(
        &mut self,
        context: &ItemContext<'_, '_>,
        link: ItemLink<'_>,
    ) -> AgendaItem {
        let ItemLink {
            ledger_line,
            ledger_depth,
            marker,
            block_link,
            path_part,
            block_id,
            index,
        } = link;
        let ledger_notes = self.ledger_notes_for(
            context.day_contents_lines,
            context.day_spans,
            context.day_fenced,
            context.entry_range,
            context.item_lines,
            ledger_line - 1,
        );
        let unresolved =
            |resolution: AgendaResolution,
             relative_target: Option<String>,
             warning: Option<String>| AgendaItem {
                index,
                ledger_line,
                ledger_depth,
                marker: marker.as_str().to_string(),
                block_link: block_link.to_string(),
                resolution,
                relative_target,
                line: None,
                block_id: link.block_id.to_string(),
                text: None,
                status_symbol: None,
                status_name: None,
                status_type: None,
                lines: Vec::new(),
                lines_truncated: 0,
                ledger_notes: ledger_notes.clone(),
                warning,
            };
        let filter_cleared = marker == TaskLinkMarker::Embedded;
        let day_path = context.day_path;
        let (target_path, relative_target) = if path_part.is_empty() {
            (day_path.to_path_buf(), self.relative_target(day_path))
        } else {
            match self.resolver.resolve(day_path, path_part) {
                LinkResolution::Found(path) => {
                    let absolute = if path.is_absolute() {
                        path
                    } else {
                        self.bob_dir.join(&path)
                    };
                    let relative = self.relative_target(&absolute);
                    (absolute, relative)
                }
                LinkResolution::Ambiguous => {
                    return unresolved(
                        AgendaResolution::AmbiguousNote,
                        None,
                        Some(bounded_warning(format!(
                            "{block_link} has an ambiguous note basename"
                        ))),
                    );
                }
                LinkResolution::Missing => {
                    return unresolved(
                        AgendaResolution::MissingNote,
                        None,
                        Some(bounded_warning(format!(
                            "{block_link} does not resolve to a vault note"
                        ))),
                    );
                }
            }
        };
        let Some((target_contents, scan)) =
            self.note_for(&target_path, filter_cleared)
        else {
            return unresolved(
                AgendaResolution::Unreadable,
                Some(relative_target.clone()),
                Some(bounded_warning(format!(
                    "{block_link} resolves to {relative_target}, which cannot be read"
                ))),
            );
        };
        match scan.by_block_id(block_id) {
            note_tasks::BlockIdLookup::Found(task) => {
                let task = task.clone();
                let text = if filter_cleared {
                    close_task_text(&task.description)
                } else {
                    task.description.clone()
                };
                let (lines, lines_truncated) = self.task_block_lines(
                    &target_contents,
                    &task,
                    filter_cleared,
                );
                AgendaItem {
                    index,
                    ledger_line,
                    ledger_depth,
                    marker: marker.as_str().to_string(),
                    block_link: block_link.to_string(),
                    resolution: AgendaResolution::Resolved,
                    relative_target: Some(relative_target),
                    line: Some(task.line_index + 1),
                    block_id: block_id.to_string(),
                    text: Some(text),
                    status_symbol: Some(task.status_symbol),
                    status_name: Some(task.status_name.clone()),
                    status_type: Some(
                        status_type_label(task.status_type).to_string(),
                    ),
                    lines,
                    lines_truncated,
                    ledger_notes,
                    warning: None,
                }
            }
            note_tasks::BlockIdLookup::NotATask {
                line_index,
                excerpt,
            } => unresolved(
                AgendaResolution::NotATask,
                Some(relative_target.clone()),
                Some(bounded_warning(format!(
                    "{relative_target} contains ^{block_id} on a non-task line {} ({excerpt})",
                    line_index + 1
                ))),
            ),
            note_tasks::BlockIdLookup::Duplicate(count) => unresolved(
                AgendaResolution::DuplicateBlock,
                Some(relative_target.clone()),
                Some(bounded_warning(format!(
                    "{relative_target} contains duplicate ^{block_id} IDs ({count} lines)"
                ))),
            ),
            note_tasks::BlockIdLookup::Missing => unresolved(
                AgendaResolution::MissingBlock,
                Some(relative_target.clone()),
                Some(bounded_warning(format!(
                    "{relative_target} has no task with block ID ^{block_id}"
                ))),
            ),
        }
    }

    /// Display lines for one resolved task's block (the task line's own
    /// extent minus the task line), with blank lines dropped and the
    /// payload bounded at [`MAX_TASK_BLOCK_LINES`].
    fn task_block_lines(
        &self,
        target_contents: &str,
        task: &note_tasks::NoteTask,
        filter_cleared: bool,
    ) -> (Vec<AgendaLine>, usize) {
        let spans = line_spans(target_contents);
        let line_text: Vec<&str> = spans.iter().map(|span| span.text).collect();
        // `block_end` is the byte end of the block's last line: every
        // later line fully inside the block ends at or before it.
        let mut end_index = task.line_index;
        for (index, span) in spans.iter().enumerate() {
            if index > task.line_index && span.end <= task.block_end {
                end_index = index;
            }
        }
        if end_index <= task.line_index {
            return (Vec::new(), 0);
        }
        let range = task.line_index + 1..end_index + 1;
        let fenced = markdown::fenced_lines(&line_text, 0..line_text.len());
        let depths = block_depths(&line_text, task.line_index, range.clone());
        let global_filter = self.global_filter_for(filter_cleared);
        let mut lines = build_lines(
            &line_text,
            &fenced,
            &depths,
            range,
            global_filter,
            filter_cleared,
        );
        let mut truncated = 0;
        if lines.len() > MAX_TASK_BLOCK_LINES {
            truncated = lines.len() - MAX_TASK_BLOCK_LINES;
            lines.truncate(MAX_TASK_BLOCK_LINES);
        }
        (lines, truncated)
    }

    /// Descendant lines of one link line in the ledger that are not items
    /// themselves, with depth relative to the link line. Lines nested
    /// under another item belong to that item, so walking up past an
    /// item line stops the search.
    fn ledger_notes_for(
        &self,
        line_text: &[&str],
        spans: &[LineSpan<'_>],
        fenced: &BTreeSet<usize>,
        entry_range: &Range<usize>,
        item_lines: &BTreeSet<usize>,
        link_index: usize,
    ) -> Vec<AgendaLine> {
        let link_line = link_index + 1;
        let mut raw = Vec::new();
        for index in (link_index + 1)..entry_range.end {
            if item_lines.contains(&(index + 1)) {
                continue;
            }
            if fenced.contains(&index) {
                continue;
            }
            let mut cursor = index;
            let mut belongs = false;
            while let Some(parent) =
                nearest_shallower_list_item_parent(spans, cursor)
            {
                if parent + 1 == link_line {
                    belongs = true;
                    break;
                }
                if item_lines.contains(&(parent + 1)) {
                    // Nested under another item: that item owns it.
                    break;
                }
                cursor = parent;
            }
            if belongs {
                raw.push(index);
            }
        }
        if raw.is_empty() {
            return Vec::new();
        }
        let start = link_index + 1;
        let end = raw.last().copied().unwrap_or(link_index) + 1;
        let depths = block_depths(line_text, link_index, start..end);
        let mut lines = Vec::new();
        for index in raw.into_iter() {
            let depth = depths
                .get(index.saturating_sub(start))
                .copied()
                .unwrap_or(1)
                .max(1);
            if let Some(line) = build_one_line(
                line_text[index],
                depth,
                fenced.contains(&index),
                &self.settings.global_filter,
                false,
            ) {
                lines.push(line);
            }
        }
        tag_log_subtrees(&mut lines);
        lines
    }
}

/// The agenda date: the day file's date, or today when the file name is
/// not a date.
pub(crate) fn agenda_date(day_file: &Path) -> NaiveDate {
    if let Some(file_name) = day_file.file_name().and_then(|name| name.to_str())
        && let Some(date) = pomodoro::parse_day_file_date(file_name)
    {
        return date;
    }
    bob_env::current_datetime().date()
}

/// Build the per-entry agenda payload for every listed entry. Completed
/// entries carry the `completed` role with empty notes and items.
pub(crate) fn agenda_entries(
    notes: &mut AgendaNotes<'_>,
    contents: &str,
    entries: &[super::capture_pomodoros::PomodoroEntry],
    next_line: Option<usize>,
    date: NaiveDate,
    day_path: &Path,
) -> Vec<AgendaEntry> {
    let spans = line_spans(contents);
    let line_text: Vec<&str> = spans.iter().map(|span| span.text).collect();
    let fenced = markdown::fenced_lines(&line_text, 0..line_text.len());
    entries
        .iter()
        .map(|entry| {
            let role = agenda_role(entry, next_line);
            let entry_index = entry.line.saturating_sub(1);
            let range = sub_bullet_range(&line_text, entry_index);
            let walk: Vec<NumberedTaskLink> =
                number_task_links_in_range(&line_text, &fenced, range.clone());
            let queued: BTreeMap<usize, u32> =
                list_queued_links(contents, entry_index)
                    .into_iter()
                    .map(|link| (link.ledger_line, link.index))
                    .collect();
            let item_lines: BTreeSet<usize> =
                walk.iter().map(|link| link.line).collect();
            let retired_link_count =
                retired_count(&line_text, &fenced, range.clone());
            let depths = block_depths(&line_text, entry_index, range.clone());
            let context = ItemContext {
                day_contents_lines: &line_text,
                day_spans: &spans,
                day_fenced: &fenced,
                entry_range: &range,
                item_lines: &item_lines,
                day_path,
            };
            let mut items = Vec::with_capacity(walk.len());
            for link in &walk {
                let index = if entry.is_current {
                    Some(link.index)
                } else {
                    queued.get(&link.line).copied()
                };
                let ledger_depth = depths
                    .get((link.line - 1).saturating_sub(range.start))
                    .copied()
                    .unwrap_or(1)
                    .max(1);
                items.push(notes.resolve_item(
                    &context,
                    ItemLink {
                        ledger_line: link.line,
                        ledger_depth,
                        marker: link.marker,
                        block_link: &link.block_link,
                        path_part: &link.path_part,
                        block_id: &link.block_id,
                        index,
                    },
                ));
            }
            let entry_notes = entry_notes_for(
                &line_text,
                &spans,
                &fenced,
                &depths,
                &range,
                &item_lines,
                &retired_lines(&line_text, &fenced, range.clone()),
                &notes.settings.global_filter,
            );
            let (starts_at, ends_at) =
                agenda_datetimes(date, entry.time_range.as_deref());
            // Completed entries (`--all`) keep their datetimes and
            // retired count, but their notes and items stay empty: the
            // app never passes `--all`.
            let (entry_notes, items) = if role == AgendaRole::Completed {
                (Vec::new(), Vec::new())
            } else {
                (entry_notes, items)
            };
            AgendaEntry {
                role,
                starts_at,
                ends_at,
                retired_link_count,
                notes: entry_notes,
                items,
            }
        })
        .collect()
}

fn agenda_role(
    entry: &super::capture_pomodoros::PomodoroEntry,
    next_line: Option<usize>,
) -> AgendaRole {
    if entry.state == super::capture_pomodoros::PomodoroState::Completed {
        return AgendaRole::Completed;
    }
    if entry.is_current {
        return AgendaRole::Current;
    }
    if next_line == Some(entry.line) {
        return AgendaRole::Next;
    }
    if entry.time_range.is_some() {
        return AgendaRole::Open;
    }
    AgendaRole::Later
}

fn agenda_datetimes(
    date: NaiveDate,
    time_range: Option<&str>,
) -> (Option<String>, Option<String>) {
    let Some(range) = time_range else {
        return (None, None);
    };
    let (start_text, end_text) = match range.split_once('-') {
        Some((start, end)) => (start, end),
        None => return (None, None),
    };
    let (start_h, start_m) = match parse_hhmm(start_text) {
        Some(pair) => pair,
        None => return (None, None),
    };
    let (end_h, end_m) = match parse_hhmm(end_text) {
        Some(pair) => pair,
        None => return (None, None),
    };
    let starts_at = format!("{date}T{start_h:02}:{start_m:02}");
    let end_date = if (end_h, end_m) < (start_h, start_m) {
        date.succ_opt().unwrap_or(date)
    } else {
        date
    };
    let ends_at = format!("{end_date}T{end_h:02}:{end_m:02}");
    (Some(starts_at), Some(ends_at))
}

/// Minutes spanned by one normalized `HHMM-HHMM` ledger range, for the
/// completed summary. Overnight ranges wrap past midnight, exactly as
/// the close planner's duration helper normalizes them.
pub(crate) fn range_minutes(time_range: &str) -> Option<u64> {
    let (start_text, end_text) = time_range.split_once('-')?;
    let (start_h, start_m) = parse_hhmm(start_text)?;
    let (end_h, end_m) = parse_hhmm(end_text)?;
    Some(super::capture::normalize_minutes(
        (end_h * 60 + end_m) as i64 - (start_h * 60 + start_m) as i64,
    ))
}

fn parse_hhmm(text: &str) -> Option<(u32, u32)> {
    let digits: String = text
        .chars()
        .filter(|character| character.is_ascii_digit())
        .collect();
    if digits.len() != 4 {
        return None;
    }
    let hour = digits[..2].parse::<u32>().ok()?;
    let minute = digits[2..].parse::<u32>().ok()?;
    (hour <= 23 && minute <= 59).then_some((hour, minute))
}

/// Struck single-link lines in one entry's range: already-done links.
fn retired_lines(
    line_text: &[&str],
    fenced: &BTreeSet<usize>,
    range: Range<usize>,
) -> BTreeSet<usize> {
    let mut retired = BTreeSet::new();
    for index in range {
        if fenced.contains(&index) {
            continue;
        }
        let Some(line) = line_text.get(index) else {
            continue;
        };
        if is_retired_link(line) {
            retired.insert(index + 1);
        }
    }
    retired
}

fn retired_count(
    line_text: &[&str],
    fenced: &BTreeSet<usize>,
    range: Range<usize>,
) -> usize {
    retired_lines(line_text, fenced, range).len()
}

fn is_retired_link(line: &str) -> bool {
    let stripped = strip_pomodoro_markers(line);
    if wikilink_tokens(&stripped).len() != 1 {
        return false;
    }
    let strike_spans = strikethrough_inner_spans(&stripped);
    if strike_spans.is_empty() {
        return false;
    }
    let without_strike: String =
        stripped.split("~~").collect::<Vec<_>>().concat();
    if bare_plain_link(&without_strike).is_some()
        || bare_embedded_link(&without_strike).is_some()
    {
        return true;
    }
    // Deferred `[[T]]#` links retire the same way.
    let trimmed = without_strike.trim();
    if let Some(bare) = trimmed.strip_suffix('#') {
        return bare_plain_link(bare.trim_end()).is_some();
    }
    false
}

/// Session notes: sub-bullet lines that are neither items nor retired
/// links nor descendants of an item.
#[allow(clippy::too_many_arguments)]
fn entry_notes_for(
    line_text: &[&str],
    spans: &[LineSpan<'_>],
    fenced: &BTreeSet<usize>,
    depths: &[usize],
    range: &Range<usize>,
    item_lines: &BTreeSet<usize>,
    retired: &BTreeSet<usize>,
    global_filter: &str,
) -> Vec<AgendaLine> {
    let mut lines = Vec::new();
    for index in range.clone() {
        let line_number = index + 1;
        if item_lines.contains(&line_number) || retired.contains(&line_number) {
            continue;
        }
        if fenced.contains(&index) {
            continue;
        }
        if is_descendant_of_any_item(spans, index, item_lines) {
            continue;
        }
        let depth = depths
            .get(index.saturating_sub(range.start))
            .copied()
            .unwrap_or(1)
            .max(1);
        let Some(line) = line_text.get(index) else {
            continue;
        };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(note) =
            build_one_line(line, depth, false, global_filter, false)
        {
            lines.push(note);
        }
    }
    tag_log_subtrees(&mut lines);
    lines
}

fn is_descendant_of_any_item(
    spans: &[LineSpan<'_>],
    index: usize,
    item_lines: &BTreeSet<usize>,
) -> bool {
    let mut cursor = index;
    while let Some(parent) = nearest_shallower_list_item_parent(spans, cursor) {
        if item_lines.contains(&(parent + 1)) {
            return true;
        }
        cursor = parent;
    }
    false
}

/// Build every display line for one owner range: blanks dropped, fence
/// delimiters omitted, depths from `block_depths`.
fn build_lines(
    line_text: &[&str],
    fenced: &BTreeSet<usize>,
    depths: &[usize],
    range: Range<usize>,
    global_filter: &str,
    filter_cleared: bool,
) -> Vec<AgendaLine> {
    let mut lines = Vec::new();
    for index in range.clone() {
        let Some(line) = line_text.get(index) else {
            continue;
        };
        if line.trim().is_empty() {
            continue;
        }
        if fenced.contains(&index) && is_fence_delimiter(line) {
            continue;
        }
        let depth = depths
            .get(index.saturating_sub(range.start))
            .copied()
            .unwrap_or(1)
            .max(1);
        if let Some(built) = build_one_line(
            line,
            depth,
            fenced.contains(&index),
            global_filter,
            filter_cleared,
        ) {
            lines.push(built);
        }
    }
    tag_log_subtrees(&mut lines);
    lines
}

fn is_fence_delimiter(line: &str) -> bool {
    let trimmed = line
        .trim_start_matches([' ', '\t'])
        .trim_end_matches([' ', '\t']);
    trimmed.starts_with("```") || trimmed.starts_with("~~~")
}

fn build_one_line(
    line: &str,
    depth: usize,
    in_fence: bool,
    global_filter: &str,
    filter_cleared: bool,
) -> Option<AgendaLine> {
    if line.trim().is_empty() {
        return None;
    }
    if in_fence {
        return Some(AgendaLine {
            text: line.trim_start_matches([' ', '\t']).to_string(),
            depth,
            kind: AgendaLineKind::Code,
            status_symbol: None,
            log: None,
        });
    }
    if let Some(marker_kind) = parse_managed_task_log_marker(line) {
        let text = list_item_body(line)
            .unwrap_or_else(|| line.trim())
            .to_string();
        return Some(AgendaLine {
            text,
            depth,
            kind: AgendaLineKind::LogMarker,
            status_symbol: None,
            log: Some(AgendaLog::from_kind(marker_kind)),
        });
    }
    if let Some(body) = list_item_body(line) {
        if let Some((symbol, after)) = checkbox_parts(body) {
            let block_id = trailing_block_id_in_line(line);
            let mut text = note_tasks::clean_description(
                after,
                global_filter,
                block_id.as_deref(),
            );
            if filter_cleared {
                text = close_task_text(&text);
            }
            return Some(AgendaLine {
                text,
                depth,
                kind: AgendaLineKind::Task,
                status_symbol: Some(symbol),
                log: None,
            });
        }
        return Some(AgendaLine {
            text: body.to_string(),
            depth,
            kind: AgendaLineKind::Bullet,
            status_symbol: None,
            log: None,
        });
    }
    Some(AgendaLine {
        text: line.trim_start_matches([' ', '\t']).to_string(),
        depth,
        kind: AgendaLineKind::Text,
        status_symbol: None,
        log: None,
    })
}

/// Split a list body into its checkbox symbol and the text after it.
fn checkbox_parts(body: &str) -> Option<(char, &str)> {
    let after_open = body.strip_prefix('[')?;
    let mut chars = after_open.chars();
    let symbol = chars.next()?;
    let after_symbol = &after_open[symbol.len_utf8()..];
    let rest = after_symbol.strip_prefix(']')?;
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    Some((symbol, rest.trim_start_matches([' ', '\t'])))
}

/// Tag each line with its enclosing managed log (`work`/`schedule`):
/// the marker line and every line of its subtree. Prose that merely
/// mentions "work log" never matches, because only
/// [`parse_managed_task_log_marker`] arms the tag.
fn tag_log_subtrees(lines: &mut [AgendaLine]) {
    let mut stack: Vec<(usize, AgendaLog)> = Vec::new();
    for line in lines.iter_mut() {
        while stack.last().is_some_and(|(depth, _)| *depth >= line.depth) {
            stack.pop();
        }
        let own_kind = (line.kind == AgendaLineKind::LogMarker)
            .then_some(line.log)
            .flatten();
        if let Some((_, kind)) = stack.last() {
            line.log = Some(*kind);
        }
        if line.kind == AgendaLineKind::LogMarker {
            let kind = own_kind.or(line.log).unwrap_or(AgendaLog::Work);
            line.log = Some(kind);
            stack.push((line.depth, kind));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::capture_pomodoros::{
        next_future_pomodoro, scan, PomodoroState,
    };
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    const SETTINGS_JSON: &str = r##"{
      "globalFilter": "#task",
      "statusSettings": {
        "coreStatuses": [
          {"symbol":" ","name":"Todo","type":"TODO"},
          {"symbol":"x","name":"Done","type":"DONE"}
        ],
        "customStatuses": [
          {"symbol":"*","name":"Next","type":"TODO"}
        ]
      }
    }"##;

    fn write_settings(vault: &Path) {
        write_file(
            &vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
            SETTINGS_JSON,
        );
    }

    fn agenda_for(
        vault: &Path,
        day_contents: &str,
    ) -> (NaiveDate, Vec<AgendaEntry>) {
        let day_path = vault.join("2026/20261009.md");
        let found = scan(day_contents);
        let next_line = next_future_pomodoro(&found).map(|entry| entry.line);
        let date = agenda_date(&day_path);
        assert_eq!(date.to_string(), "2026-10-09");
        let mut notes = AgendaNotes::new(vault, &day_path, day_contents);
        let listed: Vec<_> = found
            .entries
            .iter()
            .filter(|entry| entry.state == PomodoroState::Open)
            .cloned()
            .collect();
        let built = agenda_entries(
            &mut notes,
            day_contents,
            &listed,
            next_line,
            date,
            &day_path,
        );
        (date, built)
    }

    #[test]
    fn roles_numbers_and_retired_counts() {
        let temp = TempDir::new("bob-cli-agenda-roles");
        let vault = temp.path().to_path_buf();
        write_settings(&vault);
        write_file(
            &vault.join("tasks.md"),
            concat!(
                "## Tasks\n",
                "- [ ] #task Alpha ^alpha\n",
                "- [ ] #task Beta ^beta\n",
            ),
        );
        let day = concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — FIX\n",
            "\t- [[tasks#^alpha]]\n",
            "\t- [[tasks#^beta]]#\n",
            "\t- ![[tasks#^alpha]]\n",
            "\t\t- [[tasks#^beta]]\n",
            "\t- ~~[[tasks#^alpha]]~~\n",
            "\t- session note\n",
            "- [ ] () — SASE\n",
            "\t- [[tasks#^beta]]\n",
            "\t\t- [[tasks#^alpha]]\n",
            "\t- [[tasks#^alpha]]#\n",
            "- [ ] () — LATER\n",
        );
        let (_, entries) = agenda_for(&vault, day);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].role, AgendaRole::Current);
        assert_eq!(entries[1].role, AgendaRole::Next);
        assert_eq!(entries[2].role, AgendaRole::Later);
        assert_eq!(entries[0].starts_at.as_deref(), Some("2026-10-09T09:00"));
        assert_eq!(entries[0].ends_at.as_deref(), Some("2026-10-09T09:30"));
        assert_eq!(entries[1].starts_at, None);
        // Current entry: every recognized line is numbered in order.
        let current: Vec<Option<u32>> =
            entries[0].items.iter().map(|item| item.index).collect();
        assert_eq!(current, [Some(1), Some(2), Some(3), Some(4)]);
        assert_eq!(entries[0].retired_link_count, 1);
        assert_eq!(entries[0].notes.len(), 1);
        assert_eq!(entries[0].notes[0].text, "session note");
        // Next entry: direct plain children keep `=` numbers; the nested
        // link and the deferred link are unnumbered.
        let next: Vec<Option<u32>> =
            entries[1].items.iter().map(|item| item.index).collect();
        assert_eq!(next, [Some(1), None, None]);
        assert_eq!(entries[1].items[0].marker, TaskLinkMarker::Plain.as_str());
        assert_eq!(
            entries[1].items[2].marker,
            TaskLinkMarker::Deferred.as_str()
        );
        for item in entries[0].items.iter().chain(entries[1].items.iter()) {
            assert_eq!(item.resolution, AgendaResolution::Resolved);
            assert_eq!(item.relative_target.as_deref(), Some("tasks.md"));
            assert!(item.warning.is_none());
        }
    }

    #[test]
    fn current_numbers_match_number_task_links_walk() {
        let temp = TempDir::new("bob-cli-agenda-parity");
        let vault = temp.path().to_path_buf();
        write_settings(&vault);
        write_file(&vault.join("tasks.md"), "- [ ] #task Solo ^solo\n");
        let day = concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — FIX\n",
            "\t- [[tasks#^solo]]\n",
            "\t- quick note\n",
            "\t- ![[tasks#^solo]]\n",
        );
        let spans = line_spans(day);
        let line_text: Vec<&str> = spans.iter().map(|span| span.text).collect();
        let expected = number_task_links_in_range(
            &line_text,
            &markdown::fenced_lines(&line_text, 0..line_text.len()),
            1..5,
        );
        let (_, entries) = agenda_for(&vault, day);
        let items = &entries[0].items;
        assert_eq!(items.len(), expected.len());
        for (item, link) in items.iter().zip(expected.iter()) {
            assert_eq!(item.index, Some(link.index));
            assert_eq!(item.ledger_line, link.line);
            assert_eq!(item.marker, link.marker.as_str());
            assert_eq!(item.block_id, link.block_id);
        }
    }

    #[test]
    fn resolutions_cover_every_failure_mode() {
        let temp = TempDir::new("bob-cli-agenda-resolution");
        let vault = temp.path().to_path_buf();
        write_settings(&vault);
        write_file(
            &vault.join("notes.md"),
            concat!(
                "- [ ] #task Dup ^dup\n",
                "- [ ] #task Dup again ^dup\n",
                "- [ ] #task Real ^real\n",
                "- plain bullet ^flat\n",
            ),
        );
        write_file(&vault.join("one/dup.md"), "- [ ] #task A ^x\n");
        write_file(&vault.join("two/dup.md"), "- [ ] #task B ^x\n");
        let day = concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — FIX\n",
            "\t- [[notes#^real]]\n",
            "\t- [[notes#^dup]]\n",
            "\t- [[notes#^flat]]\n",
            "\t- [[notes#^gone]]\n",
            "\t- [[missing#^gone]]\n",
            "\t- [[dup#^x]]\n",
            "\t- [[#^self]]\n",
            "- [ ] () — SASE\n",
        );
        let (_, entries) = agenda_for(&vault, day);
        let items = &entries[0].items;
        assert_eq!(items[0].resolution, AgendaResolution::Resolved);
        assert_eq!(items[0].line, Some(3));
        assert_eq!(items[1].resolution, AgendaResolution::DuplicateBlock);
        assert!(items[1].warning.as_deref().unwrap().contains("duplicate"));
        assert_eq!(items[2].resolution, AgendaResolution::NotATask);
        assert!(items[2].warning.as_deref().unwrap().contains("non-task"));
        assert_eq!(items[3].resolution, AgendaResolution::MissingBlock);
        assert!(items[3].warning.as_deref().unwrap().contains("no task"));
        assert_eq!(items[4].resolution, AgendaResolution::MissingNote);
        assert_eq!(items[4].relative_target, None);
        assert_eq!(items[5].resolution, AgendaResolution::AmbiguousNote);
        assert_eq!(items[5].relative_target, None);
        // `[[#^self]]` resolves against the daily note, which has no
        // tasks, so the block is missing there.
        assert_eq!(items[6].resolution, AgendaResolution::MissingBlock);
        assert_eq!(
            items[6].relative_target.as_deref(),
            Some("2026/20261009.md")
        );
        for item in items {
            assert!(
                item.warning.as_deref().is_some_and(|warning| {
                    !warning.is_empty() && warning.chars().count() <= 300
                }) || item.resolution == AgendaResolution::Resolved
            );
        }
    }

    #[test]
    fn unreadable_notes_fail_soft() {
        let temp = TempDir::new("bob-cli-agenda-unreadable");
        let vault = temp.path().to_path_buf();
        write_settings(&vault);
        write_file(&vault.join("tasks.md"), "- [ ] #task A ^a\n");
        let day = concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — FIX\n",
            "\t- [[tasks#^a]]\n",
        );
        // Point the link at a path the resolver finds but cannot read:
        // a directory named like a note is skipped by the direct join,
        // so instead remove read permission from a real note file.
        let target = vault.join("locked.md");
        write_file(&target, "- [ ] #task Locked ^a\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&target, fs::Permissions::from_mode(0o000))
                .expect("chmod locked note");
        }
        let day = format!("{day}\t- [[locked#^a]]\n");
        let (_, entries) = agenda_for(&vault, &day);
        let items = &entries[0].items;
        assert_eq!(items[0].resolution, AgendaResolution::Resolved);
        // Root reads through permission bits, so derive the expectation
        // from what the filesystem actually enforces.
        if fs::read_to_string(&target).is_err() {
            assert_eq!(items[1].resolution, AgendaResolution::Unreadable);
            assert_eq!(items[1].relative_target.as_deref(), Some("locked.md"));
        } else {
            assert_eq!(items[1].resolution, AgendaResolution::Resolved);
        }
    }

    #[test]
    fn logs_tag_subtrees_and_prose_is_untagged() {
        let temp = TempDir::new("bob-cli-agenda-logs");
        let vault = temp.path().to_path_buf();
        write_settings(&vault);
        write_file(
            &vault.join("tasks.md"),
            concat!(
                "## Tasks\n",
                "- [ ] #task Logged ^logged\n",
                "\t- first child\n",
                "\t- \u{1F6E0}\u{FE0F} **Work log**\n",
                "\t\t- Oct 8 — did things\n",
                "\t\t- [ ] #task nested ^nested\n",
                "\t\t\t- nested detail\n",
                "\t- **Schedule log**\n",
                "\t\t- Oct 9 — plan\n",
                "\t- prose that mentions work log stays plain\n",
            ),
        );
        let day = concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — FIX\n",
            "\t- [[tasks#^logged]]\n",
        );
        let (_, entries) = agenda_for(&vault, day);
        let lines = &entries[0].items[0].lines;
        let kinds: Vec<(&str, AgendaLineKind, Option<AgendaLog>)> = lines
            .iter()
            .map(|line| (line.text.as_str(), line.kind, line.log))
            .collect();
        assert!(kinds.iter().any(|(text, kind, log)| {
            *text == "first child"
                && *kind == AgendaLineKind::Bullet
                && log.is_none()
        }));
        let marker = lines
            .iter()
            .find(|line| line.kind == AgendaLineKind::LogMarker)
            .expect("work log marker");
        assert_eq!(marker.log, Some(AgendaLog::Work));
        let entry = lines
            .iter()
            .find(|line| line.text == "Oct 8 — did things")
            .expect("log entry");
        assert_eq!(entry.log, Some(AgendaLog::Work));
        let nested = lines
            .iter()
            .find(|line| line.text == "nested")
            .expect("nested task line");
        assert_eq!(nested.kind, AgendaLineKind::Task);
        assert_eq!(nested.status_symbol, Some(' '));
        // The nested task sits under the Work log marker, so it and its
        // detail inherit the work tag.
        assert_eq!(nested.log, Some(AgendaLog::Work));
        let detail = lines
            .iter()
            .find(|line| line.text == "nested detail")
            .expect("nested detail");
        assert_eq!(detail.log, Some(AgendaLog::Work));
        let scheduled = lines
            .iter()
            .find(|line| line.text == "Oct 9 — plan")
            .expect("schedule entry");
        assert_eq!(scheduled.log, Some(AgendaLog::Schedule));
        let prose = lines
            .iter()
            .find(|line| line.text.contains("mentions work log"))
            .expect("prose line");
        assert_eq!(prose.log, None);
    }

    #[test]
    fn nested_log_markers_keep_their_own_kind() {
        let temp = TempDir::new("bob-cli-agenda-nested-log");
        let vault = temp.path().to_path_buf();
        write_settings(&vault);
        write_file(
            &vault.join("tasks.md"),
            concat!(
                "## Tasks\n",
                "- [ ] #task Outer ^outer\n",
                "\t- \u{1F6E0}\u{FE0F} **Work log**\n",
                "\t\t- [ ] #task inner ^inner\n",
                "\t\t\t- **Schedule log**\n",
                "\t\t\t\t- Oct 9 — scheduled\n",
            ),
        );
        let day = concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — FIX\n",
            "\t- [[tasks#^outer]]\n",
        );
        let (_, entries) = agenda_for(&vault, day);
        let lines = &entries[0].items[0].lines;
        let marker = lines
            .iter()
            .find(|line| {
                line.kind == AgendaLineKind::LogMarker
                    && line.text == "**Schedule log**"
            })
            .expect("schedule marker");
        assert_eq!(marker.log, Some(AgendaLog::Schedule));
        let entry = lines
            .iter()
            .find(|line| line.text == "Oct 9 — scheduled")
            .expect("schedule entry");
        assert_eq!(entry.log, Some(AgendaLog::Schedule));
    }

    #[test]
    fn resolved_items_carry_ledger_notes() {
        let temp = TempDir::new("bob-cli-agenda-ledger-notes");
        let vault = temp.path().to_path_buf();
        write_settings(&vault);
        write_file(
            &vault.join("tasks.md"),
            concat!("## Tasks\n", "- [ ] #task Alpha ^alpha\n",),
        );
        let day = concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — FIX\n",
            "\t- [[tasks#^alpha]]\n",
            "\t\t- ledger detail\n",
        );
        let (_, entries) = agenda_for(&vault, day);
        let notes = &entries[0].items[0].ledger_notes;
        assert!(!notes.is_empty());
        assert!(notes.iter().any(|line| line.text == "ledger detail"));
    }

    #[test]
    fn open_entries_can_link_done_tasks() {
        let temp = TempDir::new("bob-cli-agenda-done-link");
        let vault = temp.path().to_path_buf();
        write_settings(&vault);
        write_file(
            &vault.join("tasks.md"),
            concat!("## Tasks\n", "- [x] #task Finished ^done\n",),
        );
        let day = concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — FIX\n",
            "\t- [[tasks#^done]]\n",
        );
        let (_, entries) = agenda_for(&vault, day);
        let item = &entries[0].items[0];
        assert_eq!(item.resolution, AgendaResolution::Resolved);
        assert_eq!(item.text.as_deref(), Some("Finished"));
        assert_eq!(item.status_symbol, Some('x'));
        assert_eq!(item.status_type.as_deref(), Some("DONE"));
    }

    #[test]
    fn task_block_lines_have_clean_text_kinds_and_depths() {
        let temp = TempDir::new("bob-cli-agenda-block-lines");
        let vault = temp.path().to_path_buf();
        write_settings(&vault);
        write_file(
            &vault.join("tasks.md"),
            concat!(
                "- [ ] #task Titled [created:: 2026-10-01] ^titled\n",
                "  - [x] sub done ^sub\n",
                "  - plain bullet\n",
                "    continuation text\n",
                "  ```md\n",
                "  code line\n",
                "  ```\n",
            ),
        );
        let day = concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — FIX\n",
            "\t- [[tasks#^titled]]\n",
        );
        let (_, entries) = agenda_for(&vault, day);
        let item = &entries[0].items[0];
        assert_eq!(item.text.as_deref(), Some("Titled"));
        assert_eq!(item.status_symbol, Some(' '));
        assert_eq!(item.status_name.as_deref(), Some("Todo"));
        assert_eq!(item.status_type.as_deref(), Some("TODO"));
        let texts: Vec<(&str, AgendaLineKind, usize)> = item
            .lines
            .iter()
            .map(|line| (line.text.as_str(), line.kind, line.depth))
            .collect();
        assert!(texts.contains(&("sub done", AgendaLineKind::Task, 1)));
        assert!(texts.contains(&("plain bullet", AgendaLineKind::Bullet, 1)));
        assert!(texts.contains(&(
            "continuation text",
            AgendaLineKind::Text,
            2
        )));
        // The fenced code line's parent chain reaches the task itself,
        // so it sits at depth 1 like any other direct continuation.
        assert!(texts.contains(&("code line", AgendaLineKind::Code, 1)));
        // Inline fields and the block id are cleaned from task titles.
        let sub = item
            .lines
            .iter()
            .find(|line| line.text == "sub done")
            .expect("sub task");
        assert_eq!(sub.status_symbol, Some('x'));
    }

    #[test]
    fn pomodoro_markers_and_prose_mix_with_links() {
        let temp = TempDir::new("bob-cli-agenda-markers");
        let vault = temp.path().to_path_buf();
        write_settings(&vault);
        write_file(&vault.join("tasks.md"), "- [ ] #task Alpha ^alpha\n");
        let day = concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — FIX\n",
            "\t- \u{1F345} [[tasks#^alpha]]\n",
            "\t- just prose, not a link\n",
            "\t- [[tasks#^alpha]] trailing prose\n",
            "\t- ![[tasks#^alpha]]\n",
        );
        let (_, entries) = agenda_for(&vault, day);
        let items = &entries[0].items;
        // The marker-prefixed link still numbers; prose and mixed lines
        // are session notes, never items.
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].index, Some(1));
        assert_eq!(items[0].block_id, "alpha");
        assert_eq!(items[1].index, Some(2));
        let notes: Vec<&str> = entries[0]
            .notes
            .iter()
            .map(|note| note.text.as_str())
            .collect();
        assert!(notes.contains(&"just prose, not a link"));
        assert!(notes.contains(&"[[tasks#^alpha]] trailing prose"));
    }

    #[test]
    fn crlf_and_missing_final_newline_resolve() {
        let temp = TempDir::new("bob-cli-agenda-crlf");
        let vault = temp.path().to_path_buf();
        write_settings(&vault);
        write_file(&vault.join("tasks.md"), "- [ ] #task Alpha ^alpha\n");
        let day =
            "## Pomodoros\r\n- [ ] (0900-0930) — FIX\r\n\t- [[tasks#^alpha]]";
        let (_, entries) = agenda_for(&vault, day);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].items.len(), 1);
        assert_eq!(entries[0].items[0].resolution, AgendaResolution::Resolved);
        assert_eq!(entries[0].items[0].text.as_deref(), Some("Alpha"));
    }

    #[test]
    fn long_task_blocks_truncate_at_150_lines() {
        let temp = TempDir::new("bob-cli-agenda-truncate");
        let vault = temp.path().to_path_buf();
        write_settings(&vault);
        let mut task_note = String::from("- [ ] #task Big ^big\n");
        for index in 0..200 {
            task_note.push_str(&format!("\t- line {index}\n"));
        }
        write_file(&vault.join("tasks.md"), &task_note);
        let day = concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — FIX\n",
            "\t- [[tasks#^big]]\n",
        );
        let (_, entries) = agenda_for(&vault, day);
        let item = &entries[0].items[0];
        assert_eq!(item.lines.len(), MAX_TASK_BLOCK_LINES);
        assert_eq!(item.lines_truncated, 50);
    }

    #[test]
    fn overnight_ranges_roll_the_end_date() {
        let temp = TempDir::new("bob-cli-agenda-overnight");
        let vault = temp.path().to_path_buf();
        write_settings(&vault);
        let day = concat!(
            "## Pomodoros\n",
            "- [ ] (2330-0010) — LATE\n",
            "- [x] (0800-0900) — DONE\n",
            "\t- [[tasks#^a]]\n",
        );
        let day_path = vault.join("2026/20261009.md");
        let found = scan(day);
        let next_line = next_future_pomodoro(&found).map(|entry| entry.line);
        let date = agenda_date(&day_path);
        let listed: Vec<_> = found.entries.to_vec();
        let mut notes = AgendaNotes::new(&vault, &day_path, day);
        let built = agenda_entries(
            &mut notes, day, &listed, next_line, date, &day_path,
        );
        assert_eq!(built[0].starts_at.as_deref(), Some("2026-10-09T23:30"));
        assert_eq!(built[0].ends_at.as_deref(), Some("2026-10-10T00:10"));
        assert_eq!(built[1].role, AgendaRole::Completed);
        assert!(built[1].notes.is_empty());
        assert!(built[1].items.is_empty());
        assert_eq!(range_minutes("0800-0900"), Some(60));
    }

    #[test]
    fn each_note_reads_once_per_filter_mode() {
        let temp = TempDir::new("bob-cli-agenda-read-count");
        let vault = temp.path().to_path_buf();
        write_settings(&vault);
        for note in ["a", "b", "c", "d"] {
            write_file(
                &vault.join(format!("{note}.md")),
                &format!("- [ ] #task Task {note} ^{note}\n"),
            );
        }
        let mut day = String::from("## Pomodoros\n");
        // 10 entries, 25 links across 4 notes, every note linked both
        // plain and embedded so both filter modes are exercised.
        let mut links = 0;
        for entry in 0..10 {
            day.push_str(&format!("- [ ] () — E{entry}\n"));
            for (position, note) in ["a", "b", "c", "d"].iter().enumerate() {
                if links >= 25 {
                    break;
                }
                // Alternate the filter mode per entry so every note is
                // linked both plain and embedded across the ledger.
                if (entry + position) % 2 == 0 {
                    day.push_str(&format!("\t- [[{note}#^{note}]]\n"));
                } else {
                    day.push_str(&format!("\t- ![[{note}#^{note}]]\n"));
                }
                links += 1;
            }
        }
        assert_eq!(links, 25);
        let day_path = vault.join("2026/20261009.md");
        let found = scan(&day);
        let next_line = next_future_pomodoro(&found).map(|entry| entry.line);
        let date = agenda_date(&day_path);
        let listed: Vec<_> = found
            .entries
            .iter()
            .filter(|entry| entry.state == PomodoroState::Open)
            .cloned()
            .collect();
        let mut notes = AgendaNotes::new(&vault, &day_path, &day);
        let built = agenda_entries(
            &mut notes, &day, &listed, next_line, date, &day_path,
        );
        assert_eq!(built.len(), 10);
        assert_eq!(notes.test_reads(), 8);
    }

    #[test]
    fn september_shaped_vault_finishes_fast() {
        let temp = TempDir::new("bob-cli-agenda-perf");
        let vault = temp.path().to_path_buf();
        write_settings(&vault);
        for note in 0..6 {
            let mut contents = String::new();
            for task in 0..13 {
                contents.push_str(&format!(
                    "- [ ] #task Task {note}-{task} ^n{note}-t{task}\n\t- detail {task}\n"
                ));
            }
            write_file(&vault.join(format!("n{note}.md")), &contents);
        }
        let mut day = String::from("## Pomodoros\n");
        for entry in 0..24 {
            if entry == 0 {
                day.push_str("- [ ] (0900-0930) — FIRST\n");
            } else {
                day.push_str(&format!("- [ ] () — E{entry}\n"));
            }
            for link in 0..3 {
                let note = (entry + link) % 6;
                let task = (entry * 3 + link) % 13;
                day.push_str(&format!("\t- [[n{note}#^n{note}-t{task}]]\n"));
            }
            day.push_str("\t- extra note line\n");
        }
        // 24 entries, 72 links plus 6 to reach the September shape.
        day.push_str("- [ ] () — EXTRA\n");
        for extra in 0..6 {
            day.push_str(&format!("\t- [[n0#^n0-t{extra}]]\n"));
        }
        let day_path = vault.join("2026/20261009.md");
        let start = SystemTime::now();
        let found = scan(&day);
        let next_line = next_future_pomodoro(&found).map(|entry| entry.line);
        let date = agenda_date(&day_path);
        let listed: Vec<_> = found
            .entries
            .iter()
            .filter(|entry| entry.state == PomodoroState::Open)
            .cloned()
            .collect();
        let mut notes = AgendaNotes::new(&vault, &day_path, &day);
        let built = agenda_entries(
            &mut notes, &day, &listed, next_line, date, &day_path,
        );
        let elapsed = start.elapsed().expect("clock");
        assert_eq!(built.len(), 25);
        assert!(
            elapsed.as_millis() < 250,
            "September-shaped agenda took {elapsed:?}"
        );
    }

    fn write_file(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().expect("file parent"))
            .expect("create file parent");
        fs::write(path, contents).expect("write file");
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before epoch")
                .as_nanos();
            let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "{prefix}-{}-{nonce}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            if let Err(error) = fs::remove_dir_all(&self.path) {
                eprintln!("failed to remove {}: {error}", self.path.display());
            }
        }
    }
}

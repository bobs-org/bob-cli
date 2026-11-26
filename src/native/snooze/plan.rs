//! Pure add/drop/wake/cleanup planners: snapshots in, postimages out.
//!
//! Contract: `docs/snooze.md` § Mutations. Callers (the guarded CLI, and
//! later capture's in-memory batch) supply explicit note snapshots; this
//! module never shells out or writes files. Same-path task and daily-note
//! edits coalesce, including shifted line offsets after a task-line stamp.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use crate::native::{
    capture::{
        leading_spaces_or_tabs_len, line_spans, list_item_body,
        nearest_shallower_list_item_parent,
    },
    capture_pomodoros::{self, PomodoroState},
    capture_task_toggle::{
        child_block_end_line, extract_line_range, insert_named_placeholder,
        insert_subtree_into_entry, insert_unnamed_placeholder, logical_lines,
        plan_task_link, pomodoro_child_indentation, reindent_subtree,
        SubtreeInsert,
    },
    pomodoro,
    pomodoro_themes::routing::{self, PlaceEntry, PlaceInput, PlaceOutcome},
    task_dependencies::block_link_spans,
    task_status_hooks,
};

use super::{
    parse::{self, SnoozeRow, SnoozeRowKind, SnoozedSection},
    state::{
        classify_rows, ClassifiedRows, ClassifyInput, ClassifyRow,
        IgnoreReason, ResolutionKind, RowState, TaskIdentity, TaskState,
    },
    when::{format_hhmm, ResolvedTime, SnoozeClock},
};

/// One file the planner rewrote. Paths are vault-relative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FilePostimage {
    pub(crate) path: PathBuf,
    pub(crate) contents: String,
}

/// Resolved facts for one task the planner may touch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskFact {
    pub(crate) identity: TaskIdentity,
    pub(crate) path: PathBuf,
    pub(crate) contents: String,
    /// Zero-based task line in `contents`.
    pub(crate) line_index: usize,
    pub(crate) state: TaskState,
    /// Effectively Blocked (checkbox or inherited project block).
    pub(crate) blocked: bool,
    pub(crate) type_key: Option<String>,
    pub(crate) type_name: Option<String>,
    /// Wikilink spelling written into the snooze row / woken ledger link.
    pub(crate) link_text: String,
}

/// Map from `(wikilink target, block id)` to a resolved identity.
pub(crate) type IdentityMap = BTreeMap<(String, String), TaskIdentity>;

/// Inputs for [`plan_add`].
#[derive(Debug, Clone)]
pub(crate) struct AddInput<'a> {
    pub(crate) day_path: &'a Path,
    pub(crate) day: &'a str,
    pub(crate) task: &'a TaskFact,
    pub(crate) when: &'a ResolvedTime,
    pub(crate) clock: SnoozeClock,
    pub(crate) identities: &'a IdentityMap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AddRefusal {
    EventTask,
    Blocked,
    Done,
    Cancelled,
    NotTask,
    NoPomodorosSection,
}

impl AddRefusal {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::EventTask => "event_task",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
            Self::NotTask => "not_task",
            Self::NoPomodorosSection => "no_pomodoros",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AddPlan {
    pub(crate) files: Vec<FilePostimage>,
    /// True when the task-file postimage equals its preimage.
    pub(crate) task_unchanged: bool,
}

/// Inputs for [`plan_drop`].
#[derive(Debug, Clone)]
pub(crate) struct DropInput<'a> {
    pub(crate) day: &'a str,
    pub(crate) identity: &'a TaskIdentity,
    pub(crate) identities: &'a IdentityMap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DropRefusal {
    NotSnoozed,
}

impl DropRefusal {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::NotSnoozed => "not_snoozed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DropPlan {
    pub(crate) day: String,
}

/// Inputs for [`plan_wake`].
#[derive(Debug, Clone)]
pub(crate) struct WakeInput<'a> {
    pub(crate) day_path: &'a Path,
    pub(crate) day: &'a str,
    pub(crate) clock: SnoozeClock,
    pub(crate) facts: &'a [TaskFact],
    pub(crate) identities: &'a IdentityMap,
    pub(crate) exempt: &'a [String],
    /// `None` wakes due live rows; `Some` wakes those identities if they
    /// are asleep or due (blocked and ignored stay put).
    pub(crate) explicit: Option<Vec<TaskIdentity>>,
    pub(crate) link_count: Option<usize>,
    pub(crate) max_links: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WakeDestination {
    pub(crate) line: usize,
    pub(crate) name: Option<String>,
    pub(crate) created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WokenEntry {
    pub(crate) identity: TaskIdentity,
    pub(crate) target: String,
    pub(crate) woke_at: String,
    pub(crate) destination: WakeDestination,
    pub(crate) over_cap: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReasonedRow {
    pub(crate) identity: Option<TaskIdentity>,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingRow {
    pub(crate) identity: TaskIdentity,
    pub(crate) state: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WakePlan {
    pub(crate) files: Vec<FilePostimage>,
    pub(crate) woken: Vec<WokenEntry>,
    pub(crate) dropped: Vec<ReasonedRow>,
    pub(crate) ignored: Vec<ReasonedRow>,
    pub(crate) pending: Vec<PendingRow>,
    pub(crate) next_wake_at: Option<String>,
    pub(crate) over_cap: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WakeRefusal {
    NoPomodorosSection,
    MultipleOpenTimedEntries,
}

impl WakeRefusal {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::NoPomodorosSection => "no_pomodoros",
            Self::MultipleOpenTimedEntries => "multiple_open_timed",
        }
    }
}

fn wake_refusal_from_insert(
    error: crate::native::capture_task_toggle::LinkRelocationError,
) -> WakeRefusal {
    use crate::native::capture_task_toggle::LinkRelocationError as E;
    match error {
        E::MultipleOpenTimedEntries => WakeRefusal::MultipleOpenTimedEntries,
        _ => WakeRefusal::NoPomodorosSection,
    }
}

/// Inputs for [`plan_cleanup`].
#[derive(Debug, Clone)]
pub(crate) struct CleanupInput<'a> {
    pub(crate) day_path: &'a Path,
    pub(crate) day: &'a str,
    pub(crate) clock: SnoozeClock,
    pub(crate) facts: &'a [TaskFact],
    pub(crate) identities: &'a IdentityMap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CleanupPlan {
    pub(crate) day: String,
}

/// Add a snooze row for `input.task` at `input.when`.
pub(crate) fn plan_add(input: &AddInput<'_>) -> Result<AddPlan, AddRefusal> {
    refuse_add_target(input.task)?;
    let mut day = input.day.to_string();
    let mut task_contents = input.task.contents.clone();
    let same = paths_equal(input.day_path, &input.task.path);
    let mut task_unchanged = true;
    if input.task.state.status == ' ' && !input.task.blocked {
        let source = if same {
            day.as_str()
        } else {
            task_contents.as_str()
        };
        let Some(plan) =
            plan_task_link(source, input.task.line_index, input.clock.date)
        else {
            return Err(AddRefusal::NotTask);
        };
        task_unchanged = plan.content == source;
        if same {
            day = plan.content;
        } else {
            task_contents = plan.content;
        }
    }

    let (notes, after_consume) = consume_live_links(
        &day,
        &input.task.identity,
        input.identities,
        &display_text(input.task),
        input.task.link_text.as_str(),
        input.when,
    );
    day = after_consume;
    day = upsert_active_row(
        &day,
        &input.task.identity,
        input.identities,
        input.when,
        input.task.link_text.as_str(),
        &notes,
    )?;

    let mut files = vec![FilePostimage {
        path: input.day_path.to_path_buf(),
        contents: day,
    }];
    if !same {
        files.push(FilePostimage {
            path: input.task.path.clone(),
            contents: task_contents,
        });
    }
    Ok(AddPlan {
        files,
        task_unchanged,
    })
}

/// Drop every unstruck active row for `identity`. History stays.
pub(crate) fn plan_drop(
    input: &DropInput<'_>,
) -> Result<DropPlan, DropRefusal> {
    let mut buf = NoteBuf::from(input.day);
    let indexes =
        active_rows_for(&buf.render(), input.identity, input.identities);
    if indexes.is_empty() {
        return Err(DropRefusal::NotSnoozed);
    }
    for (start, end) in indexes.into_iter().rev() {
        buf.remove_range(start, end);
    }
    drop_empty_section(&mut buf);
    Ok(DropPlan { day: buf.render() })
}

/// Wake due (or explicit) live rows into never-running destinations.
///
/// A wake whose never-running destination cannot be placed refuses the whole
/// batch with `WakeRefusal` and writes nothing. A wake that wakes nothing
/// (no due rows, or prune only) never refuses for a malformed ledger.
pub(crate) fn plan_wake(
    input: &WakeInput<'_>,
) -> Result<WakePlan, WakeRefusal> {
    let classified = classify_snapshot(
        input.day,
        input.day_path,
        input.clock,
        input.facts,
        input.identities,
    );
    let fact_by_id: BTreeMap<&TaskIdentity, &TaskFact> = input
        .facts
        .iter()
        .map(|fact| (&fact.identity, fact))
        .collect();
    let over_cap = match (input.link_count, input.max_links) {
        (Some(count), Some(cap)) => count >= cap,
        _ => false,
    };

    let mut waking = Vec::new();
    let mut dropped = Vec::new();
    let mut ignored = Vec::new();
    let explicit = input.explicit.as_ref();
    for (position, row) in classified.rows.iter().enumerate() {
        let explicit_hit = match (explicit, &row.identity) {
            (None, _) => false,
            (Some(wanted), Some(identity)) => wanted.contains(identity),
            (Some(_), None) => false,
        };
        let wake_it = match row.state {
            RowState::Due => explicit.is_none() || explicit_hit,
            RowState::Asleep => explicit_hit,
            RowState::Ignored(IgnoreReason::Done)
            | RowState::Ignored(IgnoreReason::Cancelled)
            | RowState::Ignored(IgnoreReason::AlreadyLinked) => {
                dropped.push(ReasonedRow {
                    identity: row.identity.clone(),
                    reason: row.state.as_str().to_string(),
                });
                false
            }
            RowState::Ignored(_) | RowState::Blocked | RowState::History => {
                if !matches!(row.state, RowState::History)
                    && (explicit.is_none() || explicit_hit)
                {
                    ignored.push(ReasonedRow {
                        identity: row.identity.clone(),
                        reason: row.state.as_str().to_string(),
                    });
                }
                false
            }
        };
        if wake_it && let Some(identity) = row.identity.clone() {
            waking.push((position, identity, row.minutes, row.hhmm.clone()));
        }
    }
    waking.sort_by_key(|(position, _, minutes, _)| {
        (minutes.unwrap_or(u16::MAX), *position)
    });
    if let Some(wanted) = explicit {
        waking.retain(|(_, identity, _, _)| wanted.contains(identity));
    }

    let parsed_lines: Vec<&str> = input.day.lines().collect();
    let section = parse::parse_section(&parsed_lines);
    let mut jobs = Vec::new();
    if let Some(section) = &section {
        for (position, identity, _minutes, hhmm) in &waking {
            let Some(row) = section
                .rows
                .iter()
                .find(|row| row.line == classified.rows[*position].line)
            else {
                continue;
            };
            let Some(fact) = fact_by_id.get(identity) else {
                continue;
            };
            let subtree = extract_row_subtree(input.day, row);
            jobs.push(WakeJob {
                identity: identity.clone(),
                hhmm: hhmm.clone().unwrap_or_else(|| format_hhmm(0)),
                link_text: fact.link_text.clone(),
                type_key: fact.type_key.clone(),
                type_name: fact.type_name.clone(),
                subtree,
            });
        }
    }

    let mut day = input.day.to_string();
    let mut woken = Vec::new();
    let mut inserted_lines: BTreeMap<String, usize> = BTreeMap::new();
    let woke_at = format_hhmm_colon(input.clock.now_minutes);
    for job in &jobs {
        let (after_create, dest, created) = resolve_or_create_destination(
            &day,
            job.type_key.as_deref(),
            job.type_name.as_deref(),
            input.exempt,
        )?;
        day = after_create;
        let key = dest_key(&dest);
        let already = inserted_lines.get(&key).copied().unwrap_or(0);
        let dest_index = dest.line.saturating_sub(1);
        let subtree =
            ledger_subtree(&day, dest_index, &job.link_text, &job.subtree);
        let position = if already == 0 {
            SubtreeInsert::FirstChild
        } else {
            SubtreeInsert::Append
        };
        let insert_at = if already == 0 {
            dest_index
        } else {
            dest_index + already
        };
        let (updated, _) = if already == 0 {
            insert_subtree_into_entry(&day, dest_index, &subtree, position)
                .map_err(wake_refusal_from_insert)?
        } else {
            let lines = line_spans(&day);
            let content =
                crate::native::capture_task_toggle::insert_lines_after(
                    &day, &lines, insert_at, &subtree,
                );
            (
                content,
                crate::native::capture_task_toggle::LinkPlacement::Inserted,
            )
        };
        day = updated;
        *inserted_lines.entry(key).or_insert(0) += subtree.len();
        let dest_after = destination_after(&day, &dest, created);
        woken.push(WokenEntry {
            identity: job.identity.clone(),
            target: job.hhmm.clone(),
            woke_at: woke_at.clone(),
            destination: dest_after,
            over_cap,
        });
    }

    day = strike_woken(&day, &jobs, input.clock.now_minutes);
    day = prune_active_rows(&day, &dropped, input.identities);

    let after = classify_snapshot(
        &day,
        input.day_path,
        input.clock,
        input.facts,
        input.identities,
    );
    let pending = pending_from(&after);
    let mut files = vec![FilePostimage {
        path: input.day_path.to_path_buf(),
        contents: day,
    }];
    let mut seen_paths = BTreeSet::new();
    seen_paths.insert(input.day_path.to_path_buf());
    for fact in input.facts {
        if !seen_paths.insert(fact.path.clone()) {
            continue;
        }
        files.push(FilePostimage {
            path: fact.path.clone(),
            contents: fact.contents.clone(),
        });
    }
    Ok(WakePlan {
        files,
        woken,
        dropped,
        ignored,
        pending,
        next_wake_at: after.next_wake_at,
        over_cap,
    })
}

/// Remove done, cancelled, and already-linked active rows. Blocked,
/// malformed, Ready, Event, and history rows stay. Capture and plugins
/// reuse this for in-memory batch cleanup.
pub(crate) fn plan_cleanup(input: &CleanupInput<'_>) -> CleanupPlan {
    let classified = classify_snapshot(
        input.day,
        input.day_path,
        input.clock,
        input.facts,
        input.identities,
    );
    let mut prune = Vec::new();
    for row in &classified.rows {
        if matches!(
            row.state,
            RowState::Ignored(IgnoreReason::Done)
                | RowState::Ignored(IgnoreReason::Cancelled)
                | RowState::Ignored(IgnoreReason::AlreadyLinked)
        ) {
            prune.push(ReasonedRow {
                identity: row.identity.clone(),
                reason: row.state.as_str().to_string(),
            });
        }
    }
    let day = prune_active_rows(input.day, &prune, input.identities);
    CleanupPlan { day }
}

struct WakeJob {
    identity: TaskIdentity,
    hhmm: String,
    link_text: String,
    type_key: Option<String>,
    type_name: Option<String>,
    subtree: String,
}

struct Dest {
    line: usize,
    name: Option<String>,
}

fn dest_key(dest: &Dest) -> String {
    dest.name.clone().unwrap_or_default()
}

fn refuse_add_target(task: &TaskFact) -> Result<(), AddRefusal> {
    if !task.state.is_task {
        return Err(AddRefusal::NotTask);
    }
    if task.state.is_event {
        return Err(AddRefusal::EventTask);
    }
    if task.blocked || task.state.status == '?' {
        return Err(AddRefusal::Blocked);
    }
    if matches!(task.state.status, 'x' | 'X') {
        return Err(AddRefusal::Done);
    }
    if task.state.status == '-' {
        return Err(AddRefusal::Cancelled);
    }
    if !matches!(task.state.status, ' ' | '*' | '/') {
        return Err(AddRefusal::NotTask);
    }
    Ok(())
}

fn display_text(task: &TaskFact) -> String {
    let line = task.contents.lines().nth(task.line_index).unwrap_or("");
    task_status_hooks::kept_notes_display_text(
        line,
        "#task",
        Some(&task.identity.block_id),
    )
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    left == right
}

fn format_hhmm_colon(minutes: u16) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

fn format_active(hhmm: &str, link: &str) -> String {
    format!("- **{hhmm}** {link}")
}

fn format_history(hhmm: &str, link: &str, woke: u16) -> String {
    format!("- ~~**{hhmm}** {link}~~ · woke {}", format_hhmm_colon(woke))
}

struct NoteBuf {
    lines: Vec<String>,
    ending: String,
    trailing: bool,
}

impl NoteBuf {
    fn from(contents: &str) -> Self {
        let ending = if contents.contains("\r\n") {
            "\r\n".to_string()
        } else {
            "\n".to_string()
        };
        Self {
            lines: contents.lines().map(str::to_string).collect(),
            ending,
            trailing: contents.ends_with('\n'),
        }
    }

    fn render(&self) -> String {
        if self.lines.is_empty() {
            return if self.trailing {
                self.ending.clone()
            } else {
                String::new()
            };
        }
        let mut out = self.lines.join(&self.ending);
        if self.trailing {
            out.push_str(&self.ending);
        }
        out
    }

    fn remove_range(&mut self, start: usize, end: usize) {
        if start >= self.lines.len() {
            return;
        }
        let end = end.min(self.lines.len().saturating_sub(1));
        if end >= start {
            self.lines.drain(start..=end);
        }
    }
}

fn child_end_of(lines: &[String], parent: usize) -> usize {
    let parent_indent = leading_spaces_or_tabs_len(&lines[parent]);
    let mut end = parent;
    let mut index = parent + 1;
    while index < lines.len() {
        if lines[index].trim().is_empty() {
            index += 1;
            continue;
        }
        if leading_spaces_or_tabs_len(&lines[index]) > parent_indent {
            end = index;
            index += 1;
            continue;
        }
        break;
    }
    end
}

struct LiveHit {
    entry_line: usize,
    line_index: usize,
    subtree_end: usize,
    dedicated: bool,
    untimed: bool,
    had_children: bool,
}

fn consume_live_links(
    day: &str,
    identity: &TaskIdentity,
    identities: &IdentityMap,
    display: &str,
    link_text: &str,
    when: &ResolvedTime,
) -> (Vec<String>, String) {
    let hits = collect_live_hits(day, identity, identities);
    let mut notes = Vec::new();
    if let Some(first) = hits.iter().find(|hit| hit.dedicated) {
        let extracted = extract_range(day, first.line_index, first.subtree_end);
        let source_indent_len = day
            .lines()
            .nth(first.line_index)
            .map(leading_spaces_or_tabs_len)
            .unwrap_or(0);
        let source_indent = day
            .lines()
            .nth(first.line_index)
            .map(|line| &line[..source_indent_len])
            .unwrap_or("");
        let reindented = reindent_subtree(&extracted, source_indent, "");
        let mut lines = logical_lines(&reindented);
        if let Some(first_line) = lines.first_mut() {
            *first_line = format_active(&when.hhmm, link_text);
        }
        notes = lines;
    }
    let mut buf = NoteBuf::from(day);
    let first_dedicated = hits.iter().position(|hit| hit.dedicated);
    for (index, hit) in hits.iter().enumerate().rev() {
        if hit.line_index >= buf.lines.len() {
            continue;
        }
        let keep_notes = Some(index) != first_dedicated
            && hit.dedicated
            && hit.subtree_end > hit.line_index;
        if keep_notes {
            let indent_len =
                leading_spaces_or_tabs_len(&buf.lines[hit.line_index]);
            let indent = buf.lines[hit.line_index][..indent_len].to_string();
            buf.lines[hit.line_index] = format!("{indent}- {display}");
        } else {
            buf.remove_range(hit.line_index, hit.subtree_end);
        }
    }
    // Hits were collected on the preimage, so entry indexes may have
    // shifted; delete currently-empty untimed open entries whose
    // headline still matches an original hit's entry headline.
    let rendered = buf.render();
    let original_lines: Vec<&str> = day.lines().collect();
    let original_headlines: BTreeSet<String> = hits
        .iter()
        .filter(|hit| hit.untimed && hit.had_children)
        .filter_map(|hit| original_lines.get(hit.entry_line).copied())
        .map(str::to_string)
        .collect();
    let mut drop_entries = Vec::new();
    let current = NoteBuf::from(&rendered);
    let scan = capture_pomodoros::scan(&rendered);
    for entry in scan.entries.iter().rev() {
        if entry.state != PomodoroState::Open || entry.time_range.is_some() {
            continue;
        }
        let index = entry.line - 1;
        if child_end_of(&current.lines, index) != index {
            continue;
        }
        let headline =
            current.lines.get(index).map(String::as_str).unwrap_or("");
        if original_headlines.contains(headline) {
            drop_entries.push(index);
        }
    }
    let mut buf = current;
    for index in drop_entries {
        if index < buf.lines.len() {
            buf.remove_range(index, index);
        }
    }
    (notes, buf.render())
}

fn collect_live_hits(
    day: &str,
    identity: &TaskIdentity,
    identities: &IdentityMap,
) -> Vec<LiveHit> {
    let scan = capture_pomodoros::scan(day);
    let lines = line_spans(day);
    let mut hits = Vec::new();
    for entry in &scan.entries {
        if entry.state != PomodoroState::Open {
            continue;
        }
        let entry_line = entry.line - 1;
        let child_end = child_block_end_line(&lines, entry_line);
        let had_children = child_end > entry_line;
        let untimed = entry.time_range.is_none();
        if child_end < entry_line + 1 {
            continue;
        }
        for line_index in entry_line + 1..=child_end {
            if nearest_shallower_list_item_parent(&lines, line_index)
                != Some(entry_line)
            {
                continue;
            }
            let text = lines[line_index].text;
            for span in block_link_spans(text) {
                if span.struck {
                    continue;
                }
                let Some(resolved) =
                    resolve_link(&span.target, &span.block_id, identities)
                else {
                    continue;
                };
                if &resolved != identity {
                    continue;
                }
                let token = &text[span.token_start..span.end];
                let dedicated =
                    list_item_body(text).map(str::trim_end) == Some(token);
                let subtree_end = child_block_end_line(&lines, line_index);
                hits.push(LiveHit {
                    entry_line,
                    line_index,
                    subtree_end,
                    dedicated,
                    untimed,
                    had_children,
                });
                break;
            }
        }
    }
    hits
}

fn resolve_link(
    target: &str,
    block: &str,
    identities: &IdentityMap,
) -> Option<TaskIdentity> {
    identities
        .get(&(target.to_string(), block.to_string()))
        .cloned()
}

fn extract_range(contents: &str, start: usize, end: usize) -> String {
    let lines = line_spans(contents);
    if start >= lines.len() {
        return String::new();
    }
    let end = end.min(lines.len() - 1);
    extract_line_range(contents, &lines, start, end).to_string()
}

fn extract_row_subtree(day: &str, row: &SnoozeRow) -> String {
    let end = row.children_end.saturating_sub(1).max(row.line_index);
    extract_range(day, row.line_index, end)
}

fn upsert_active_row(
    day: &str,
    identity: &TaskIdentity,
    identities: &IdentityMap,
    when: &ResolvedTime,
    link_text: &str,
    notes: &[String],
) -> Result<String, AddRefusal> {
    let mut buf = NoteBuf::from(day);
    let parsed: Vec<&str> = buf.lines.iter().map(String::as_str).collect();
    let existing = parse::parse_section(&parsed);
    if existing.is_none() {
        insert_snoozed_heading(&mut buf)?;
    }
    let rendered = buf.render();
    let parsed: Vec<&str> = rendered.lines().collect();
    let section =
        parse::parse_section(&parsed).ok_or(AddRefusal::NoPomodorosSection)?;
    let mut buf = NoteBuf::from(&rendered);
    let active = section.rows.iter().find(|row| {
        row.kind == SnoozeRowKind::Active
            && row_matches(row, identity, identities)
    });
    let history = section.rows.iter().rev().find(|row| {
        row.kind == SnoozeRowKind::History
            && row_matches(row, identity, identities)
    });
    if let Some(row) = active {
        let start = row.line_index;
        let end = row.children_end.saturating_sub(1).max(row.line_index);
        buf.remove_range(start, end);
        return Ok(insert_sorted_row(&buf.render(), when, link_text, notes));
    }
    if let Some(row) = history {
        let start = row.line_index;
        let end = row.children_end.saturating_sub(1).max(row.line_index);
        buf.remove_range(start, end);
        return Ok(insert_sorted_row(&buf.render(), when, link_text, notes));
    }
    Ok(insert_sorted_row(&buf.render(), when, link_text, notes))
}

fn insert_snoozed_heading(buf: &mut NoteBuf) -> Result<(), AddRefusal> {
    let rendered = buf.render();
    let lines: Vec<&str> = rendered.lines().collect();
    let range = pomodoro::pomodoros_section_range(&lines)
        .ok_or(AddRefusal::NoPomodorosSection)?;
    let insert_at = range.end;
    let mut block = Vec::new();
    if insert_at > 0
        && buf
            .lines
            .get(insert_at - 1)
            .is_some_and(|line| !line.trim().is_empty())
    {
        block.push(String::new());
    }
    block.push("## Snoozed".to_string());
    block.push(String::new());
    // Keep a blank between the new section and the following H2.
    if insert_at < buf.lines.len() {
        block.push(String::new());
    }
    if insert_at >= buf.lines.len() {
        buf.lines.extend(block);
        buf.trailing = true;
    } else {
        for (offset, line) in block.into_iter().enumerate() {
            buf.lines.insert(insert_at + offset, line);
        }
    }
    Ok(())
}

fn insert_sorted_row(
    day: &str,
    when: &ResolvedTime,
    link_text: &str,
    notes: &[String],
) -> String {
    let mut buf = NoteBuf::from(day);
    let parsed: Vec<&str> = buf.lines.iter().map(String::as_str).collect();
    let Some(section) = parse::parse_section(&parsed) else {
        return day.to_string();
    };
    let mut new_lines = Vec::new();
    if notes.first().is_some_and(|line| line.starts_with("- **")) {
        new_lines.extend(notes.iter().cloned());
    } else {
        new_lines.push(format_active(&when.hhmm, link_text));
        new_lines.extend(notes.iter().cloned());
    }
    let insert_at =
        sorted_insert_index(&buf, &section, when.minutes, link_text);
    for (offset, line) in new_lines.into_iter().enumerate() {
        if insert_at + offset >= buf.lines.len() {
            buf.lines.push(line);
        } else {
            buf.lines.insert(insert_at + offset, line);
        }
    }
    buf.trailing = true;
    buf.render()
}

fn sorted_insert_index(
    buf: &NoteBuf,
    section: &SnoozedSection,
    minutes: u16,
    link_text: &str,
) -> usize {
    for row in &section.rows {
        if row.kind != SnoozeRowKind::Active {
            continue;
        }
        let row_minutes = row.minutes.unwrap_or(u16::MAX);
        let row_link = row_link_text(row);
        if (row_minutes, row_link.as_str()) > (minutes, link_text) {
            return row.line_index;
        }
    }
    if let Some(last) = section
        .rows
        .iter()
        .filter(|row| row.kind == SnoozeRowKind::Active)
        .next_back()
    {
        return last.children_end;
    }
    let mut at = section.header_line + 1;
    if buf.lines.get(at).is_some_and(|line| line.trim().is_empty()) {
        at += 1;
    }
    at.min(buf.lines.len())
}

fn row_link_text(row: &SnoozeRow) -> String {
    let target = row.target.as_deref().unwrap_or("");
    let block = row.block_id.as_deref().unwrap_or("");
    if target.is_empty() {
        format!("[[#^{block}]]")
    } else {
        format!("[[{target}#^{block}]]")
    }
}

fn row_matches(
    row: &SnoozeRow,
    identity: &TaskIdentity,
    identities: &IdentityMap,
) -> bool {
    let Some(block) = row.block_id.as_deref() else {
        return false;
    };
    let target = row.target.as_deref().unwrap_or("");
    resolve_link(target, block, identities).as_ref() == Some(identity)
}

fn active_rows_for(
    day: &str,
    identity: &TaskIdentity,
    identities: &IdentityMap,
) -> Vec<(usize, usize)> {
    let lines: Vec<&str> = day.lines().collect();
    let Some(section) = parse::parse_section(&lines) else {
        return Vec::new();
    };
    section
        .rows
        .iter()
        .filter(|row| {
            row.kind == SnoozeRowKind::Active
                && row_matches(row, identity, identities)
        })
        .map(|row| {
            (
                row.line_index,
                row.children_end.saturating_sub(1).max(row.line_index),
            )
        })
        .collect()
}

fn drop_empty_section(buf: &mut NoteBuf) {
    let rendered = buf.render();
    let lines: Vec<&str> = rendered.lines().collect();
    let Some(section) = parse::parse_section(&lines) else {
        return;
    };
    if !section.rows.is_empty() {
        return;
    }
    let prose = lines
        .get(section.content.clone())
        .into_iter()
        .flatten()
        .any(|line| !line.trim().is_empty());
    if prose {
        return;
    }
    let mut end = section.content.end.saturating_sub(1);
    if section.content.start >= section.content.end {
        end = section.header_line;
    }
    let at_eof = section.content.end == lines.len();
    buf.remove_range(section.header_line, end.max(section.header_line));
    if at_eof {
        while buf.lines.last().is_some_and(|line| line.trim().is_empty()) {
            buf.lines.pop();
        }
        buf.trailing = true;
    }
}

fn classify_snapshot(
    day: &str,
    day_path: &Path,
    clock: SnoozeClock,
    facts: &[TaskFact],
    identities: &IdentityMap,
) -> ClassifiedRows {
    let lines: Vec<&str> = day.lines().collect();
    let Some(section) = parse::parse_section(&lines) else {
        return ClassifiedRows {
            rows: Vec::new(),
            next_wake_at: None,
        };
    };
    let facts_by_id: BTreeMap<&TaskIdentity, &TaskFact> =
        facts.iter().map(|fact| (&fact.identity, fact)).collect();
    let live = live_identities(day, identities);
    let mut rows = Vec::new();
    for row in &section.rows {
        let identity = row.block_id.as_deref().and_then(|block| {
            resolve_link(row.target.as_deref().unwrap_or(""), block, identities)
        });
        let (resolution, identity, task, has_live_link) = match &identity {
            None => (ResolutionKind::Missing, None, None, false),
            Some(identity) => match facts_by_id.get(identity) {
                None => (
                    ResolutionKind::Missing,
                    Some(identity.clone()),
                    None,
                    live.contains(identity),
                ),
                Some(fact) => {
                    let mut state = fact.state;
                    if fact.blocked {
                        state.status = '?';
                    }
                    (
                        ResolutionKind::Found,
                        Some(identity.clone()),
                        Some(state),
                        live.contains(identity),
                    )
                }
            },
        };
        rows.push(ClassifyRow {
            line: row.line,
            minutes: row.minutes,
            is_history: row.kind == SnoozeRowKind::History,
            malformed: row.kind == SnoozeRowKind::Malformed,
            resolution,
            identity,
            task,
            has_live_link,
        });
    }
    let _ = day_path;
    classify_rows(&ClassifyInput {
        today: clock.date,
        note_date: clock.date,
        now_minutes: clock.now_minutes,
        zone: clock.zone,
        rows,
    })
}

fn live_identities(
    day: &str,
    identities: &IdentityMap,
) -> BTreeSet<TaskIdentity> {
    let scan = capture_pomodoros::scan(day);
    let lines = line_spans(day);
    let mut found = BTreeSet::new();
    for entry in &scan.entries {
        if entry.state != PomodoroState::Open {
            continue;
        }
        let entry_line = entry.line - 1;
        let child_end = child_block_end_line(&lines, entry_line);
        if child_end < entry_line + 1 {
            continue;
        }
        for line_index in entry_line + 1..=child_end {
            for span in block_link_spans(lines[line_index].text) {
                if span.struck {
                    continue;
                }
                if let Some(identity) =
                    resolve_link(&span.target, &span.block_id, identities)
                {
                    found.insert(identity);
                }
            }
        }
    }
    found
}

fn pending_from(classified: &ClassifiedRows) -> Vec<PendingRow> {
    classified
        .rows
        .iter()
        .filter_map(|row| {
            let identity = row.identity.clone()?;
            match row.state {
                RowState::Asleep | RowState::Due | RowState::Blocked => {
                    Some(PendingRow {
                        identity,
                        state: row.state.as_str().to_string(),
                    })
                }
                _ => None,
            }
        })
        .collect()
}

fn resolve_or_create_destination(
    day: &str,
    type_key: Option<&str>,
    type_name: Option<&str>,
    exempt: &[String],
) -> Result<(String, Dest, bool), WakeRefusal> {
    let scan = capture_pomodoros::scan(day);
    let entries: Vec<PlaceEntry<'_>> = scan
        .entries
        .iter()
        .map(|entry| {
            let open = entry.state == PomodoroState::Open;
            PlaceEntry {
                name: entry.name.as_deref(),
                open,
                running: open && entry.time_range.is_some(),
            }
        })
        .collect();
    match type_key {
        Some(key) => match routing::place(&PlaceInput {
            type_key: Some(key),
            type_name,
            entries: &entries,
            exempt,
            already_queued: false,
            never_running: true,
        }) {
            PlaceOutcome::Entry(index) => {
                let entry = &scan.entries[index];
                Ok((
                    day.to_string(),
                    Dest {
                        line: entry.line,
                        name: entry.name.clone(),
                    },
                    false,
                ))
            }
            PlaceOutcome::Create { name } => {
                let (content, created, canonical) =
                    insert_named_placeholder(day, &name, true)
                        .map_err(wake_refusal_from_insert)?;
                Ok((
                    content,
                    Dest {
                        line: created + 1,
                        name: Some(canonical),
                    },
                    true,
                ))
            }
            PlaceOutcome::NoType | PlaceOutcome::AlreadyQueued => {
                untyped_destination(day, &scan)
            }
        },
        None => untyped_destination(day, &scan),
    }
}

fn untyped_destination(
    day: &str,
    scan: &capture_pomodoros::PomodoroScan,
) -> Result<(String, Dest, bool), WakeRefusal> {
    if let Some(entry) = scan.entries.iter().find(|entry| {
        entry.state == PomodoroState::Open && entry.time_range.is_none()
    }) {
        return Ok((
            day.to_string(),
            Dest {
                line: entry.line,
                name: entry.name.clone(),
            },
            false,
        ));
    }
    let (content, created) =
        insert_unnamed_placeholder(day).map_err(wake_refusal_from_insert)?;
    Ok((
        content,
        Dest {
            line: created + 1,
            name: None,
        },
        true,
    ))
}

fn ledger_subtree(
    day: &str,
    dest_entry_index: usize,
    link_text: &str,
    snooze_subtree: &str,
) -> Vec<String> {
    let lines = line_spans(day);
    let line_texts: Vec<&str> = lines.iter().map(|line| line.text).collect();
    let section = pomodoro::pomodoros_section_range(&line_texts)
        .unwrap_or(dest_entry_index..dest_entry_index + 1);
    let dest_end = child_block_end_line(&lines, dest_entry_index);
    let indent = pomodoro_child_indentation(
        &lines,
        dest_entry_index,
        dest_end,
        section.start,
        section.end,
    );
    let rewritten = rewrite_snooze_to_link(snooze_subtree, link_text);
    let reindented = reindent_subtree(&rewritten, "", &indent);
    logical_lines(&reindented)
}

fn rewrite_snooze_to_link(subtree: &str, link_text: &str) -> String {
    let mut lines = logical_lines(subtree);
    if let Some(first) = lines.first_mut() {
        *first = format!("- {link_text}");
    }
    let ending = if subtree.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut out = lines.join(ending);
    if subtree.ends_with('\n') && !out.ends_with('\n') {
        out.push_str(ending);
    }
    out
}

fn destination_after(
    day: &str,
    planned: &Dest,
    created: bool,
) -> WakeDestination {
    let scan = capture_pomodoros::scan(day);
    let entry = scan
        .entries
        .iter()
        .find(|entry| {
            entry.state == PomodoroState::Open
                && entry.name == planned.name
                && (created || entry.line == planned.line)
        })
        .or_else(|| {
            scan.entries.iter().find(|entry| {
                entry.state == PomodoroState::Open && entry.name == planned.name
            })
        });
    WakeDestination {
        line: entry.map(|entry| entry.line).unwrap_or(planned.line),
        name: planned.name.clone(),
        created,
    }
}

fn strike_woken(day: &str, jobs: &[WakeJob], woke: u16) -> String {
    let mut buf = NoteBuf::from(day);
    let mut ranges: Vec<(usize, usize, String, String)> = Vec::new();
    let parsed: Vec<&str> = buf.lines.iter().map(String::as_str).collect();
    let Some(section) = parse::parse_section(&parsed) else {
        return day.to_string();
    };
    let mut claimed = BTreeSet::new();
    for job in jobs {
        let Some(row) = section.rows.iter().find(|row| {
            row.kind == SnoozeRowKind::Active
                && row_link_text(row) == job.link_text
                && claimed.insert(row.line_index)
        }) else {
            continue;
        };
        ranges.push((
            row.line_index,
            row.children_end.saturating_sub(1).max(row.line_index),
            job.hhmm.clone(),
            job.link_text.clone(),
        ));
    }
    ranges.sort_by_key(|(start, _, _, _)| *start);
    for (start, end, hhmm, link) in ranges.into_iter().rev() {
        if start >= buf.lines.len() {
            continue;
        }
        buf.lines[start] = format_history(&hhmm, &link, woke);
        if end > start {
            buf.remove_range(start + 1, end);
        }
    }
    buf.render()
}

fn prune_active_rows(
    day: &str,
    prune: &[ReasonedRow],
    identities: &IdentityMap,
) -> String {
    let wanted: BTreeSet<TaskIdentity> = prune
        .iter()
        .filter_map(|row| row.identity.clone())
        .collect();
    if wanted.is_empty() {
        return day.to_string();
    }
    let mut buf = NoteBuf::from(day);
    let mut ranges = Vec::new();
    for identity in &wanted {
        ranges.extend(active_rows_for(&buf.render(), identity, identities));
    }
    ranges.sort_by_key(|(start, _)| *start);
    ranges.dedup();
    for (start, end) in ranges.into_iter().rev() {
        buf.remove_range(start, end);
    }
    drop_empty_section(&mut buf);
    buf.render()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::capture_task_toggle::move_subtree_to_entry_at;
    use crate::native::snooze::vectors::{load_fixture, rust_vectors};
    use crate::native::snooze::when::parse_snooze_when;
    use std::str::FromStr as _;

    fn minutes_of(text: &str) -> u16 {
        let (left, right) = text.split_once(':').expect("HH:MM");
        left.parse::<u16>().expect("hour") * 60
            + right.parse::<u16>().expect("minute")
    }

    fn date_of(text: &str) -> chrono::NaiveDate {
        chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").expect("date")
    }

    fn identity(route: &str, block: &str) -> TaskIdentity {
        TaskIdentity {
            route: PathBuf::from(route),
            block_id: block.to_string(),
        }
    }

    fn fact_from(
        value: &serde_json::Value,
        id: &str,
    ) -> (TaskFact, IdentityMap) {
        let route = value["route"].as_str().unwrap_or("job.md");
        let block = value["block"].as_str().unwrap_or("x");
        let target = value["target"].as_str().unwrap_or("job");
        let status = value["status"]
            .as_str()
            .and_then(|s| s.chars().next())
            .unwrap_or('*');
        let fact = TaskFact {
            identity: identity(route, block),
            path: PathBuf::from(value["path"].as_str().unwrap_or(route)),
            contents: value["contents"]
                .as_str()
                .unwrap_or("- [*] #task Item ^x\n")
                .to_string(),
            line_index: value["line"]
                .as_u64()
                .map(|n| n.saturating_sub(1) as usize)
                .unwrap_or(0),
            state: TaskState {
                status,
                is_task: value["is_task"].as_bool().unwrap_or(true),
                is_event: value["is_event"].as_bool().unwrap_or(false),
            },
            blocked: value["blocked"].as_bool().unwrap_or(false),
            type_key: value["type_key"].as_str().map(str::to_string),
            type_name: value["type_name"].as_str().map(str::to_string),
            link_text: value["link"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| format!("[[{target}#^{block}]]")),
        };
        let mut identities = IdentityMap::new();
        identities.insert(
            (target.to_string(), block.to_string()),
            fact.identity.clone(),
        );
        if let Some(extra) = value["identities"].as_array() {
            for item in extra {
                let t = item["target"].as_str().unwrap_or("");
                let b = item["block"].as_str().expect(id);
                let r = item["route"].as_str().unwrap_or(t);
                identities
                    .insert((t.to_string(), b.to_string()), identity(r, b));
            }
        }
        (fact, identities)
    }

    fn clock_of(input: &serde_json::Value) -> SnoozeClock {
        SnoozeClock {
            date: date_of(input["date"].as_str().unwrap_or("2026-11-25")),
            now_minutes: minutes_of(input["now"].as_str().unwrap_or("13:00")),
            zone: chrono_tz::Tz::from_str(
                input["zone"].as_str().unwrap_or("UTC"),
            )
            .expect("zone"),
        }
    }

    fn day_path() -> PathBuf {
        PathBuf::from("2026/20261125.md")
    }

    #[test]
    fn first_child_insert_lands_above_existing_children() {
        let before = "## Pomodoros\n\n- [ ] () — WRITE\n\t- [[job#^old]]\n";
        let (after, _) = insert_subtree_into_entry(
            before,
            2,
            &["\t- [[job#^new]]".to_string()],
            SubtreeInsert::FirstChild,
        )
        .expect("insert");
        let write = after.find("- [ ] () — WRITE").expect("entry");
        let new = after.find("[[job#^new]]").expect("new");
        let old = after.find("[[job#^old]]").expect("old");
        assert!(write < new && new < old, "{after}");

        let before_move = "## Pomodoros\n\n- [ ] () — WRITE\n\t- [[job#^old]]\n- [ ] () — FIX\n\t- [[job#^new]]\n";
        let (moved, _) = move_subtree_to_entry_at(
            before_move,
            5,
            5,
            2,
            SubtreeInsert::FirstChild,
        )
        .expect("move");
        let write = moved.find("- [ ] () — WRITE").expect("entry");
        let new = moved.find("[[job#^new]]").expect("new");
        let old = moved.find("[[job#^old]]").expect("old");
        let fix = moved.find("- [ ] () — FIX").expect("fix");
        assert!(write < new && new < old && old < fix, "{moved}");
    }

    fn wake_setup(
        day: &str,
        type_key: Option<&str>,
        type_name: Option<&str>,
    ) -> (Vec<TaskFact>, IdentityMap, SnoozeClock) {
        let fact = TaskFact {
            identity: identity("jobs.md", "job"),
            path: PathBuf::from("jobs.md"),
            contents: "- [*] #task Review the launch plan ^job\n".to_string(),
            line_index: 0,
            state: TaskState {
                status: '*',
                is_task: true,
                is_event: false,
            },
            blocked: false,
            type_key: type_key.map(str::to_string),
            type_name: type_name.map(str::to_string),
            link_text: "[[jobs#^job]]".to_string(),
        };
        let mut identities = IdentityMap::new();
        identities.insert(
            ("jobs".to_string(), "job".to_string()),
            fact.identity.clone(),
        );
        let clock = SnoozeClock {
            date: date_of("2026-10-08"),
            now_minutes: minutes_of("12:00"),
            zone: chrono_tz::Tz::from_str("America/New_York").expect("zone"),
        };
        let _ = day;
        (vec![fact], identities, clock)
    }

    #[test]
    fn wake_refuses_when_no_pomodoros_section() {
        let day = "# 2026-10-08\n\n## Snoozed\n- **1100** [[jobs#^job]]\n  - keep this note\n";
        let (facts, identities, clock) = wake_setup(day, None, None);
        let before = day.to_string();
        let result = plan_wake(&WakeInput {
            day_path: &day_path(),
            day,
            clock,
            facts: &facts,
            identities: &identities,
            exempt: &[],
            explicit: None,
            link_count: None,
            max_links: None,
        });
        let Err(refusal) = result else {
            panic!("expected no_pomodoros refusal");
        };
        assert_eq!(refusal, WakeRefusal::NoPomodorosSection);
        assert_eq!(refusal.as_str(), "no_pomodoros");
        assert_eq!(day, before, "refusal leaves no postimage");
    }

    #[test]
    fn wake_refuses_typed_destination_with_two_running_pomodoros() {
        let day = "## Pomodoros\n\n- [ ] (**1100-1200** [t:: 60m]) — WORK\n- [ ] (**1130-1230** [t:: 60m]) — BUILD\n\n## Snoozed\n\n- **1130** [[jobs#^job]]\n  - keep this note\n";
        let (facts, identities, clock) =
            wake_setup(day, Some("WRITE"), Some("WRITE"));
        let before = day.to_string();
        let result = plan_wake(&WakeInput {
            day_path: &day_path(),
            day,
            clock,
            facts: &facts,
            identities: &identities,
            exempt: &[],
            explicit: None,
            link_count: None,
            max_links: None,
        });
        let Err(refusal) = result else {
            panic!("expected multiple_open_timed refusal");
        };
        assert_eq!(refusal, WakeRefusal::MultipleOpenTimedEntries);
        assert_eq!(refusal.as_str(), "multiple_open_timed");
        assert_eq!(day, before, "refusal leaves no postimage");
    }

    #[test]
    fn wake_untyped_with_two_running_pomodoros_lands_after_first() {
        let day = "## Pomodoros\n\n- [ ] (**1100-1200** [t:: 60m]) — WORK\n- [ ] (**1130-1230** [t:: 60m]) — BUILD\n\n## Snoozed\n\n- **1130** [[jobs#^job]]\n  - keep this note\n";
        let (facts, identities, clock) = wake_setup(day, None, None);
        let plan = plan_wake(&WakeInput {
            day_path: &day_path(),
            day,
            clock,
            facts: &facts,
            identities: &identities,
            exempt: &[],
            explicit: None,
            link_count: None,
            max_links: None,
        })
        .expect("untyped wake succeeds with two running entries");
        assert_eq!(plan.woken.len(), 1, "one row wakes");
        let dest = &plan.woken[0].destination;
        assert_eq!(dest.name, None, "unnamed destination");
        assert!(dest.created, "placeholder is created");
        let postimage = &plan
            .files
            .iter()
            .find(|file| file.path == day_path())
            .expect("day postimage")
            .contents;
        let running: Vec<usize> = postimage
            .lines()
            .enumerate()
            .filter(|(_, line)| line.contains("**1100-1200**"))
            .map(|(index, _)| index)
            .collect();
        let woken_link = postimage
            .lines()
            .enumerate()
            .find(|(_, line)| {
                line.contains("[[jobs#^job]]") && !line.contains("~~")
            })
            .map(|(index, _)| index)
            .expect("woken link lands in the ledger");
        let running_index = running.into_iter().next().expect("running entry");
        assert!(
            woken_link > running_index,
            "woken link sits after the first timed entry:\n{postimage}"
        );
        assert!(
            !postimage
                .lines()
                .nth(woken_link)
                .unwrap_or_default()
                .contains("**1100-1200**"),
            "woken link is never inside the timed entry itself:\n{postimage}"
        );
    }

    #[test]
    fn snooze_wake_cleanup_vectors() {
        let fixture = load_fixture().expect("load snooze fixture");
        let snooze = rust_vectors(&fixture, "snooze").expect("snooze");
        let wake = rust_vectors(&fixture, "wake").expect("wake");
        let cleanup = rust_vectors(&fixture, "cleanup").expect("cleanup");
        assert!(!snooze.is_empty(), "snooze group must carry cases");
        assert!(!wake.is_empty(), "wake group must carry cases");
        assert!(!cleanup.is_empty(), "cleanup group must carry cases");
        for vector in snooze {
            match vector.mode.as_str() {
                "add" => run_snooze_vector(vector),
                "drop" => run_drop_vector(vector),
                other => panic!("{}: unknown snooze mode {other}", vector.id),
            }
        }
        for vector in wake {
            run_wake_vector(vector);
        }
        for vector in cleanup {
            run_cleanup_vector(vector);
        }
    }

    fn run_drop_vector(vector: &crate::native::snooze::vectors::SnoozeVector) {
        let id = vector.id.as_str();
        assert_eq!(vector.mode, "drop", "{id}: drop mode is drop");
        let (fact, identities) = fact_from(&vector.input["task"], id);
        if let Some(refusal) = vector.expected["refusal"].as_str() {
            let Err(got) = plan_drop(&DropInput {
                day: vector.input["day"].as_str().expect(id),
                identity: &fact.identity,
                identities: &identities,
            }) else {
                panic!("{id}: expected refusal {refusal}");
            };
            assert_eq!(got.as_str(), refusal, "{id}: refusal");
            return;
        }
        let plan = plan_drop(&DropInput {
            day: vector.input["day"].as_str().expect(id),
            identity: &fact.identity,
            identities: &identities,
        })
        .unwrap_or_else(|error| panic!("{id}: drop refused {error:?}"));
        assert_eq!(
            plan.day,
            vector.expected["day"].as_str().expect(id),
            "{id}: day postimage\nGOT:\n{}\nWANT:\n{}",
            plan.day,
            vector.expected["day"].as_str().unwrap_or("")
        );
    }

    fn run_snooze_vector(
        vector: &crate::native::snooze::vectors::SnoozeVector,
    ) {
        let id = vector.id.as_str();
        assert_eq!(vector.mode, "add", "{id}: snooze mode is add");
        let clock = clock_of(&vector.input);
        let (fact, identities) = fact_from(&vector.input["task"], id);
        if let Some(refusal) = vector.expected["refusal"].as_str() {
            let when = parse_snooze_when(
                vector.input["when"].as_str().unwrap_or("1530"),
                clock,
            );
            let Err(got) = (match when {
                Ok(resolved) => plan_add(&AddInput {
                    day_path: &day_path(),
                    day: vector.input["day"].as_str().expect(id),
                    task: &fact,
                    when: &resolved,
                    clock,
                    identities: &identities,
                }),
                Err(_) => Err(AddRefusal::NotTask),
            }) else {
                panic!("{id}: expected refusal {refusal}");
            };
            assert_eq!(got.as_str(), refusal, "{id}: refusal");
            return;
        }
        let resolved =
            parse_snooze_when(vector.input["when"].as_str().expect(id), clock)
                .unwrap_or_else(|error| panic!("{id}: when {error:?}"));
        let plan = plan_add(&AddInput {
            day_path: &day_path(),
            day: vector.input["day"].as_str().expect(id),
            task: &fact,
            when: &resolved,
            clock,
            identities: &identities,
        })
        .unwrap_or_else(|error| panic!("{id}: add refused {error:?}"));
        let day = plan
            .files
            .iter()
            .find(|file| file.path == day_path())
            .expect(id);
        assert_eq!(
            day.contents,
            vector.expected["day"].as_str().expect(id),
            "{id}: day postimage"
        );
        if let Some(expected_task) = vector.expected["task"].as_str() {
            let task = plan
                .files
                .iter()
                .find(|file| file.path == fact.path)
                .or(plan.files.first())
                .expect(id);
            assert_eq!(task.contents, expected_task, "{id}: task postimage");
        }
        if let Some(unchanged) = vector.expected["task_unchanged"].as_bool() {
            assert_eq!(plan.task_unchanged, unchanged, "{id}: task_unchanged");
        }
    }

    fn facts_from(
        input: &serde_json::Value,
        id: &str,
    ) -> (Vec<TaskFact>, IdentityMap) {
        let mut identities = IdentityMap::new();
        let mut facts = Vec::new();
        let list = input["tasks"]
            .as_array()
            .cloned()
            .or_else(|| input.get("task").cloned().map(|v| vec![v]))
            .unwrap_or_default();
        for value in &list {
            let (fact, extra) = fact_from(value, id);
            identities.extend(extra);
            facts.push(fact);
        }
        if let Some(extra) = input["identities"].as_array() {
            for item in extra {
                let t = item["target"].as_str().unwrap_or("");
                let b = item["block"].as_str().expect(id);
                let r = item["route"].as_str().unwrap_or(t);
                identities
                    .insert((t.to_string(), b.to_string()), identity(r, b));
            }
        }
        (facts, identities)
    }

    fn run_wake_vector(vector: &crate::native::snooze::vectors::SnoozeVector) {
        let id = vector.id.as_str();
        assert_eq!(vector.mode, "wake", "{id}: wake mode is wake");
        let clock = clock_of(&vector.input);
        let (facts, identities) = facts_from(&vector.input, id);
        let explicit = vector.input["explicit"].as_array().map(|items| {
            items
                .iter()
                .map(|item| {
                    identity(
                        item["route"].as_str().unwrap_or("job.md"),
                        item["block"].as_str().expect(id),
                    )
                })
                .collect::<Vec<_>>()
        });
        if let Some(refusal) = vector.expected["refusal"].as_str() {
            let Err(got) = plan_wake(&WakeInput {
                day_path: &day_path(),
                day: vector.input["day"].as_str().expect(id),
                clock,
                facts: &facts,
                identities: &identities,
                exempt: &[],
                explicit,
                link_count: vector.input["link_count"]
                    .as_u64()
                    .map(|n| n as usize),
                max_links: vector.input["max_links"]
                    .as_u64()
                    .map(|n| n as usize),
            }) else {
                panic!("{id}: expected refusal {refusal}");
            };
            assert_eq!(got.as_str(), refusal, "{id}: refusal");
            return;
        }
        let plan = plan_wake(&WakeInput {
            day_path: &day_path(),
            day: vector.input["day"].as_str().expect(id),
            clock,
            facts: &facts,
            identities: &identities,
            exempt: &[],
            explicit,
            link_count: vector.input["link_count"].as_u64().map(|n| n as usize),
            max_links: vector.input["max_links"].as_u64().map(|n| n as usize),
        })
        .unwrap_or_else(|error| {
            panic!("{id}: wake refused {}", error.as_str())
        });

        let day = plan
            .files
            .iter()
            .find(|file| file.path == day_path())
            .expect(id);
        assert_eq!(
            day.contents,
            vector.expected["day"].as_str().expect(id),
            "{id}: day postimage\nGOT:\n{}\nWANT:\n{}",
            day.contents,
            vector.expected["day"].as_str().unwrap_or("")
        );
        if vector.expected["tasks_unchanged"].as_bool() == Some(true) {
            let mut seen = BTreeSet::new();
            for fact in &facts {
                if paths_equal(&day_path(), &fact.path) {
                    continue;
                }
                if !seen.insert(fact.path.clone()) {
                    continue;
                }
                let got = plan
                    .files
                    .iter()
                    .find(|file| file.path == fact.path)
                    .expect(id);
                assert_eq!(got.contents, fact.contents, "{id}: task bytes");
            }
        }
        if let Some(woken) = vector.expected["woken"].as_array() {
            assert_eq!(plan.woken.len(), woken.len(), "{id}: woken count");
            for (got, want) in plan.woken.iter().zip(woken) {
                assert_eq!(
                    got.identity.block_id,
                    want["block"].as_str().expect(id),
                    "{id}: woken block"
                );
                if let Some(name) = want["destination"].as_str() {
                    assert_eq!(
                        got.destination.name.as_deref(),
                        Some(name),
                        "{id}: dest name"
                    );
                }
                if let Some(created) = want["created"].as_bool() {
                    assert_eq!(
                        got.destination.created, created,
                        "{id}: created"
                    );
                }
            }
        }
        if let Some(over) = vector.expected["over"].as_bool() {
            assert_eq!(plan.over_cap, over, "{id}: over_cap");
        }
        if vector.expected["second_wake_empty"].as_bool() == Some(true) {
            let again = plan_wake(&WakeInput {
                day_path: &day_path(),
                day: &day.contents,
                clock,
                facts: &facts,
                identities: &identities,
                exempt: &[],
                explicit: None,
                link_count: None,
                max_links: None,
            })
            .unwrap_or_else(|error| {
                panic!("{id}: second wake refused {}", error.as_str())
            });
            assert!(again.woken.is_empty(), "{id}: second wake");
        }
    }

    fn run_cleanup_vector(
        vector: &crate::native::snooze::vectors::SnoozeVector,
    ) {
        let id = vector.id.as_str();
        assert_eq!(vector.mode, "cleanup", "{id}: cleanup mode is cleanup");
        let clock = clock_of(&vector.input);
        let (facts, identities) = facts_from(&vector.input, id);
        let plan = plan_cleanup(&CleanupInput {
            day_path: &day_path(),
            day: vector.input["day"].as_str().expect(id),
            clock,
            facts: &facts,
            identities: &identities,
        });
        assert_eq!(
            plan.day,
            vector.expected["day"].as_str().expect(id),
            "{id}: day postimage\nGOT:\n{}\nWANT:\n{}",
            plan.day,
            vector.expected["day"].as_str().unwrap_or("")
        );
    }
}

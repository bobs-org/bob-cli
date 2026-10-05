//! Vault-wide completable-task catalog for the `!` picker.
//!
//! A read-only scanner that lists every open task in the vault annotated
//! with its exact note identity, a Bob-authored `!note:block-id`
//! replacement, and today's Task Link placement (running, worked,
//! queued, or noted). The `execute` phase reuses the same note walk,
//! locator, replacement, and recurring rules, so the picker can never
//! offer a task capture would refuse.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};

use serde::Serialize;

use super::{
    capture::line_spans,
    capture_dependency_tasks::{self, DependencyTaskResult},
    capture_language::claimed_task_complete_tokens,
    capture_link_tasks,
    capture_pomodoro_close::{
        range_is_struck, strikethrough_inner_spans, strip_pomodoro_markers,
        wikilink_tokens,
    },
    capture_pomodoros::{self, PomodoroState},
    capture_task_toggle, markdown,
    note_tasks::TaskStatusType,
    pomodoro, task_complete, task_dependencies, task_fields,
    vault_links::{NoteIndex, VaultLinkResolver},
};

/// Disabled reason for recurring rows: capture refuses these, so the
/// picker never inserts them. The text matches the `execute` refusal.
pub(crate) const RECURRING_DISABLED_REASON: &str =
    "Recurring — complete it in Obsidian so Tasks writes the next occurrence";

/// Disabled reason for tasks another `!` item in the same draft already
/// names.
pub(crate) const ALREADY_SELECTED_DISABLED_REASON: &str =
    "Already in this draft";

/// Which bucket a completable task lists under in the `!` picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CompletableGroup {
    Today,
    InProgress,
    Next,
    Open,
}

/// Human group label for shell completion and human output. Shell
/// groups use plain words (`in progress`, not `in_progress`).
pub(crate) fn group_label(group: CompletableGroup) -> &'static str {
    match group {
        CompletableGroup::Today => "today",
        CompletableGroup::InProgress => "in progress",
        CompletableGroup::Next => "next",
        CompletableGroup::Open => "open",
    }
}

/// Today's Task Link placement for a catalog task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TodayRole {
    Running,
    Worked,
    Queued,
    Noted,
}

pub(crate) fn role_label(role: TodayRole) -> &'static str {
    match role {
        TodayRole::Running => "running",
        TodayRole::Worked => "worked",
        TodayRole::Queued => "queued",
        TodayRole::Noted => "noted",
    }
}

/// Entry state behind a today placement. `running` is the single open
/// timed entry, `completed` a finished entry, and `queued` any other
/// open entry; `noted` links sit outside `## Pomodoros` and carry no
/// entry at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TodayPomodoroStatus {
    Running,
    Completed,
    Queued,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TodayPomodoro {
    pub(crate) line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) time_range: Option<String>,
    pub(crate) status: TodayPomodoroStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TodayInfo {
    pub(crate) role: TodayRole,
    /// The winning entry: the running entry, the most recent worked
    /// entry, or the earliest queued entry. Omitted for `noted`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) pomodoro: Option<TodayPomodoro>,
    /// Distinct Pomodoro entries today that link the task. Links
    /// outside `## Pomodoros` name no entry, so a noted-only task
    /// reports zero.
    pub(crate) sessions: usize,
    /// First day-file line of the winning placement's links: ledger
    /// order inside an entry. Never serialized; only the picker order
    /// reads it.
    #[serde(skip_serializing)]
    pub(crate) sort_link: i64,
}

/// One open vault task the `!` picker can name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletableTask {
    /// Exact vault-relative note path including extension.
    pub(crate) note_path: String,
    /// Short human display form (unique basename or path without
    /// `.md`), shared with the `&` catalog.
    pub(crate) locator: String,
    pub(crate) block_id: Option<String>,
    pub(crate) block_id_suggestions: Vec<String>,
    pub(crate) status_symbol: char,
    pub(crate) status_name: String,
    pub(crate) status_type: TaskStatusType,
    pub(crate) text: String,
    pub(crate) section: Option<String>,
    pub(crate) depth: usize,
    pub(crate) line: usize,
    pub(crate) task_ref: String,
    pub(crate) hidden: bool,
    /// First strict `YYYY-MM-DD` scheduled value on the task line, if
    /// any. Unlike the `:` picker this never pulls forward: it is only
    /// displayed.
    pub(crate) scheduled: Option<String>,
    /// A `[repeat:: …]`, `(repeat:: …)`, or `🔁` line, which `execute`
    /// refuses. Recurring rows stay visible with a disabled reason and
    /// sort last.
    pub(crate) recurring: bool,
    pub(crate) group: CompletableGroup,
    /// Today's Task Link placement, when today's daily note links the
    /// task.
    pub(crate) today: Option<TodayInfo>,
}

/// Discovery output: today-ordered candidates plus the note index for
/// Bob-authored `!` replacements. Unreadable notes yield the same
/// bounded warnings as the `&` catalog and never destroy the good
/// results. A missing day file means no today rows and no warning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletableResult {
    pub(crate) tasks: Vec<CompletableTask>,
    pub(crate) warnings: Vec<String>,
    pub(crate) index: NoteIndex,
}

/// Scan every task-bearing vault note for completable candidates.
///
/// `dep` is the vault-wide `&` discovery this catalog reuses (note
/// walk, exclusions, locators, and ID suggestions); `day_file` is
/// today's ledger path, so tests can pass a temp ledger without
/// touching the shared `BOB_DAY_FILE` override. Only open tasks
/// (Ready, Blocked, Next, In Progress) are listed.
pub(crate) fn discover(
    bob_dir: &Path,
    dep: &DependencyTaskResult,
    day_file: &Path,
) -> CompletableResult {
    let open_keys: HashSet<(String, String)> = dep
        .tasks
        .iter()
        .filter(|task| {
            task.open
                && task_complete::is_completable_status(task.status_symbol)
                && task.block_id.is_some()
        })
        .map(|task| {
            (
                task.note_path.clone(),
                task.block_id.clone().expect("filtered to identified tasks"),
            )
        })
        .collect();
    let today = annotate_today(bob_dir, day_file, &open_keys);
    let mut line_cache: HashMap<String, Vec<String>> = HashMap::new();
    let mut tasks = Vec::new();
    for task in dep.tasks.iter().filter(|task| {
        task.open && task_complete::is_completable_status(task.status_symbol)
    }) {
        let raw_line = raw_task_line(bob_dir, &mut line_cache, task);
        let scheduled = raw_line.as_deref().and_then(scheduled_on);
        let recurring = raw_line
            .as_deref()
            .is_some_and(task_complete::is_recurring_task_line);
        let key = (
            task.note_path.clone(),
            task.block_id.clone().unwrap_or_default(),
        );
        let group = if today.contains_key(&key) {
            CompletableGroup::Today
        } else if task.status_symbol == '/' {
            CompletableGroup::InProgress
        } else if task.status_symbol == '*' {
            CompletableGroup::Next
        } else {
            CompletableGroup::Open
        };
        tasks.push(CompletableTask {
            note_path: task.note_path.clone(),
            locator: task.locator.clone(),
            block_id: task.block_id.clone(),
            block_id_suggestions: task.block_id_suggestions.clone(),
            status_symbol: task.status_symbol,
            status_name: task.status_name.clone(),
            status_type: task.status_type,
            text: task.text.clone(),
            section: task.section.clone(),
            depth: task.depth,
            line: task.line,
            task_ref: task.task_ref.clone(),
            hidden: task.hidden,
            scheduled,
            recurring,
            group,
            today: today.get(&key).cloned(),
        });
    }
    CompletableResult {
        tasks,
        warnings: dep.warnings.clone(),
        index: dep.index.clone(),
    }
}

/// One cached raw line per task-bearing note, so scheduled and
/// recurring facts come from the authoritative line text in a single
/// read per note. Notes that fail to read keep `None`/`false`: the
/// `&` discovery already warned about them.
fn raw_task_line(
    bob_dir: &Path,
    cache: &mut HashMap<String, Vec<String>>,
    task: &capture_dependency_tasks::DependencyTask,
) -> Option<String> {
    let lines = cache.entry(task.note_path.clone()).or_insert_with(|| {
        fs::read_to_string(bob_dir.join(Path::new(&task.note_path)))
            .map(|contents| {
                contents.lines().map(str::to_string).collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });
    lines.get(task.line - 1).cloned()
}

/// First strict `YYYY-MM-DD` scheduled value on a raw task line, in
/// line order.
fn scheduled_on(raw_line: &str) -> Option<String> {
    task_fields::inline_fields(raw_line, "scheduled")
        .iter()
        .find_map(|field| {
            task_fields::parse_strict_calendar_date(&field.value)
                .map(|_| field.value.clone())
        })
}

/// One resolved Task Link sighting in today's daily note.
struct SightedLink {
    entry_line: usize,
    entry_name: Option<String>,
    entry_time_range: Option<String>,
    entry_completed: bool,
    entry_running: bool,
    link_line: usize,
}

/// Today's Task Link placement for every open catalog task.
///
/// Any unstruck block link — plain or embedded, alias allowed, `🍅`
/// markers stripped — counts when it sits outside fenced code and
/// Depends-On lines and resolves with `VaultLinkResolver` to an open
/// task. Links under the running entry read `running`, under a
/// completed entry `worked`, under any other open entry `queued`, and
/// outside `## Pomodoros` `noted`. A task keeps its strongest role
/// (running above worked above queued above noted); a worked task
/// keeps its most recent entry.
fn annotate_today(
    bob_dir: &Path,
    day_file: &Path,
    open: &HashSet<(String, String)>,
) -> HashMap<(String, String), TodayInfo> {
    let mut placed: HashMap<(String, String), TodayInfo> = HashMap::new();
    let Ok(contents) = fs::read_to_string(day_file) else {
        return placed;
    };
    let lines: Vec<&str> = contents.lines().collect();
    let line_texts = lines.clone();
    let scan = capture_pomodoros::scan(&contents);
    let section = pomodoro::pomodoros_section_range(&line_texts);
    let fenced = markdown::fenced_lines(&lines, 0..lines.len());
    let spans = line_spans(&contents);
    let child_ends: Vec<usize> = scan
        .entries
        .iter()
        .map(|entry| {
            capture_task_toggle::child_block_end_line(&spans, entry.line - 1)
        })
        .collect();
    let current = day_file.strip_prefix(bob_dir).unwrap_or(day_file);
    let resolver = VaultLinkResolver::new(bob_dir);
    let mut sightings: HashMap<(String, String), Vec<SightedLink>> =
        HashMap::new();
    for (line_index, line) in lines.iter().enumerate() {
        if fenced.contains(&line_index) {
            continue;
        }
        let stripped = strip_pomodoro_markers(line);
        if task_dependencies::is_dependency_line(&stripped) {
            continue;
        }
        let owner = section.as_ref().and_then(|range| {
            if !range.contains(&line_index) {
                return None;
            }
            owner_entry(&scan.entries, &child_ends, line_index)
        });
        let strike_spans = strikethrough_inner_spans(&stripped);
        for token in wikilink_tokens(&stripped) {
            if range_is_struck(token.start, token.end, &strike_spans) {
                continue;
            }
            let resolved = match resolver.resolve(current, &token.path_part) {
                super::vault_links::LinkResolution::Found(path) => path,
                _ => continue,
            };
            let note_path = resolved.to_string_lossy().replace('\\', "/");
            let key = (note_path, token.block_id.clone());
            if !open.contains(&key) {
                continue;
            }
            let sighting = match owner {
                Some(entry) => SightedLink {
                    entry_line: entry.line,
                    entry_name: entry.name.clone(),
                    entry_time_range: entry.time_range.clone(),
                    entry_completed: entry.state == PomodoroState::Completed,
                    entry_running: entry.is_current,
                    link_line: line_index + 1,
                },
                None => SightedLink {
                    entry_line: 0,
                    entry_name: None,
                    entry_time_range: None,
                    entry_completed: false,
                    entry_running: false,
                    link_line: line_index + 1,
                },
            };
            sightings.entry(key).or_default().push(sighting);
        }
    }
    for (key, links) in &sightings {
        placed.insert(key.clone(), place_task(links));
    }
    placed
}

/// The nearest Pomodoro entry at or above `line_index` whose child
/// block still covers the line: the entry's own line counts as its
/// own. Gap lines inside the section belong to nobody and read noted.
fn owner_entry<'a>(
    entries: &'a [capture_pomodoros::PomodoroEntry],
    child_ends: &[usize],
    line_index: usize,
) -> Option<&'a capture_pomodoros::PomodoroEntry> {
    entries
        .iter()
        .zip(child_ends.iter())
        .filter(|(entry, end)| {
            entry.line - 1 <= line_index && line_index <= **end
        })
        .map(|(entry, _)| entry)
        .next_back()
}

/// Fold one task's sightings into its strongest placement.
fn place_task(links: &[SightedLink]) -> TodayInfo {
    let owned: Vec<&SightedLink> =
        links.iter().filter(|link| link.entry_line > 0).collect();
    let role = if owned.iter().any(|link| link.entry_running) {
        TodayRole::Running
    } else if owned.iter().any(|link| link.entry_completed) {
        TodayRole::Worked
    } else if !owned.is_empty() {
        TodayRole::Queued
    } else {
        TodayRole::Noted
    };
    let sessions: HashSet<usize> =
        owned.iter().map(|link| link.entry_line).collect();
    let winner = match role {
        TodayRole::Running => owned
            .iter()
            .filter(|link| link.entry_running)
            .min_by_key(|link| (link.entry_line, link.link_line))
            .copied(),
        TodayRole::Worked => owned
            .iter()
            .filter(|link| link.entry_completed)
            .max_by_key(|link| (link.entry_line, link.link_line))
            .copied(),
        TodayRole::Queued => owned
            .iter()
            .min_by_key(|link| (link.entry_line, link.link_line))
            .copied(),
        TodayRole::Noted => None,
    };
    // Ledger order inside the winning entry (or the document for
    // noted): the first winning link line.
    let sort_link = match role {
        TodayRole::Noted => links
            .iter()
            .map(|link| link.link_line as i64)
            .min()
            .unwrap_or(0),
        _ => {
            let winner_entry = winner.map(|link| link.entry_line).unwrap_or(0);
            links
                .iter()
                .filter(|link| {
                    link.entry_line == winner_entry && role_of(link) == role
                })
                .map(|link| link.link_line as i64)
                .min()
                .unwrap_or(0)
        }
    };
    TodayInfo {
        role,
        pomodoro: winner.map(|link| TodayPomodoro {
            line: link.entry_line,
            name: link.entry_name.clone(),
            time_range: link.entry_time_range.clone(),
            status: if link.entry_running {
                TodayPomodoroStatus::Running
            } else if link.entry_completed {
                TodayPomodoroStatus::Completed
            } else {
                TodayPomodoroStatus::Queued
            },
        }),
        sessions: sessions.len(),
        sort_link,
    }
}

/// The role one sighting carries on its own.
fn role_of(link: &SightedLink) -> TodayRole {
    if link.entry_line == 0 {
        TodayRole::Noted
    } else if link.entry_running {
        TodayRole::Running
    } else if link.entry_completed {
        TodayRole::Worked
    } else {
        TodayRole::Queued
    }
}

/// Raw searchable fields for a completable task: the cleaned
/// description, locator, `locator:block-id` and block ID (identified
/// tasks only), full note path, section, and today Pomodoro name. The
/// shared tiered matcher scores these exactly like the `:` picker's
/// fields.
pub(crate) fn completable_search_fields(task: &CompletableTask) -> Vec<String> {
    let mut fields = vec![task.text.clone(), task.locator.clone()];
    if let Some(block_id) = &task.block_id {
        fields.push(format!("{}:{block_id}", task.locator));
        fields.push(block_id.clone());
    }
    fields.push(task.note_path.clone());
    if let Some(section) = &task.section {
        fields.push(section.clone());
    }
    if let Some(name) = task
        .today
        .as_ref()
        .and_then(|today| today.pomodoro.as_ref())
        .and_then(|pomodoro| pomodoro.name.as_ref())
    {
        fields.push(name.clone());
    }
    fields
}

/// Canonical empty-query order key: today rows first (running in
/// ledger order, then worked most-recent-first in ledger order inside
/// each entry, then queued in ledger order, then noted in document
/// order), then In Progress and Next by note path and line, then all
/// other open tasks by note path and document order. Within each today
/// Pomodoro entry (and within the `noted` group), visible rows come
/// first, hidden rows sink below them, and recurring rows sink last;
/// the link line, note path, and line break every remaining tie.
fn canonical_key(
    task: &CompletableTask,
) -> (u8, i64, i64, u8, u8, i64, &str, usize) {
    let section: u8 = match task.group {
        CompletableGroup::Today => 0,
        CompletableGroup::InProgress => 1,
        CompletableGroup::Next => 2,
        CompletableGroup::Open => 3,
    };
    let (role_rank, entry_key, link_key) = match task.today.as_ref() {
        Some(today) => {
            let entry_line = today
                .pomodoro
                .as_ref()
                .map(|entry| entry.line as i64)
                .unwrap_or(0);
            match today.role {
                TodayRole::Running => (0, entry_line, today.sort_link),
                TodayRole::Worked => (1, -entry_line, today.sort_link),
                TodayRole::Queued => (2, entry_line, today.sort_link),
                TodayRole::Noted => (3, 0, today.sort_link),
            }
        }
        None => (0, 0, 0),
    };
    (
        section,
        role_rank,
        entry_key,
        u8::from(task.recurring),
        u8::from(task.hidden),
        link_key,
        task.note_path.as_str(),
        task.line,
    )
}

/// Order completable tasks for the picker: query matches first
/// (today matches by score, then all other matches by score, ties
/// keep canonical order), or the plain canonical order for an empty
/// query. Every whitespace-separated term must match.
pub(crate) fn order_for_picker<'a>(
    tasks: &'a [CompletableTask],
    query: &str,
) -> Vec<&'a CompletableTask> {
    if query.trim().is_empty() {
        let mut ordered: Vec<&'a CompletableTask> = tasks.iter().collect();
        ordered.sort_by(|left, right| {
            canonical_key(left).cmp(&canonical_key(right))
        });
        return ordered;
    }
    let mut scored: Vec<(&'a CompletableTask, u32)> = tasks
        .iter()
        .filter_map(|task| {
            capture_link_tasks::match_score(
                &completable_search_fields(task),
                query,
            )
            .map(|score| (task, score))
        })
        .collect();
    // Today matches stay on top at any score; inside each partition
    // the score dominates and the canonical key breaks ties. The
    // sort is stable, so full ties keep catalog (path/line) order.
    scored.sort_by(|left, right| {
        let (left_task, left_score) = left;
        let (right_task, right_score) = right;
        let left_today = u8::from(left_task.group != CompletableGroup::Today);
        let right_today = u8::from(right_task.group != CompletableGroup::Today);
        left_today
            .cmp(&right_today)
            .then_with(|| right_score.cmp(left_score))
            .then_with(|| {
                canonical_key(left_task).cmp(&canonical_key(right_task))
            })
    });
    scored.into_iter().map(|(task, _)| task).collect()
}

/// Exact `(note_path, block_id)` pairs named by other claimed `!`
/// items in the same draft: the active item (the one holding
/// `cursor`) never marks its own task selected, but a second item
/// naming the same task still does. Unresolvable note identities are
/// skipped: only tasks the picker could list count.
pub(crate) fn draft_selected(
    dep: &DependencyTaskResult,
    bob_dir: &Path,
    raw_text: &str,
    cursor: usize,
) -> HashSet<(String, String)> {
    let mut selected = HashSet::new();
    for (start, end, note, block_id) in claimed_task_complete_tokens(raw_text) {
        if start <= cursor && cursor <= end {
            continue;
        }
        if let Ok(resolved) = capture_dependency_tasks::resolve_dependency_note(
            bob_dir, dep, &note,
        ) {
            selected.insert((
                resolved.to_string_lossy().replace('\\', "/"),
                block_id,
            ));
        }
    }
    selected
}

#[cfg(test)]
mod tests;

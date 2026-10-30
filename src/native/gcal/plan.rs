//! Pure `bob gcal sync` planner: snapshot plus journal state in,
//! `SyncPlan{contents, records, report}` out.
//!
//! The plan inserts new rows in time order (never reordering existing
//! lines), adopts crash-recovery rows by their `📅` URL, tombstones
//! journaled rows whose line is gone, and appends each eligible MEET
//! link once. When the computed contents equal the input, the plan
//! reports no change and carries no records.

use std::{
    collections::{BTreeMap, HashSet},
    ops::Range,
};

use crate::native::{
    capture::line_spans,
    capture_pomodoros::{self, PomodoroState},
    capture_task_toggle::{
        child_block_end_line, insert_link_into_entry,
        insert_named_placeholder_after_last_open,
    },
    event_task,
    pomodoro::pomodoros_section_range,
};

use super::{
    journal::{occurrence_key, OccurrenceState},
    render::{render_context_row, render_task_row, ContextRowInput},
};

/// One included occurrence with its decided rendering.
#[derive(Debug, Clone)]
pub(crate) struct PlannedOccurrence {
    pub calendar_id: String,
    pub event_id: String,
    pub ical_uid: Option<String>,
    pub series_id: Option<String>,
    /// Bare title for block-id minting.
    pub id_hint: String,
    pub task: bool,
    pub time_label: String,
    pub title: String,
    pub html_link: Option<String>,
    pub join_link: Option<String>,
    pub location: Option<String>,
    pub root: Option<String>,
    pub source: Option<String>,
    /// Context-row qualifier (`maybe`, `free`, `day 2 of 2`); tasks ignore it.
    pub qualifier: Option<String>,
    /// Short SHA-256 hashes of each managed span, for the journal.
    pub spans: BTreeMap<String, String>,
    /// Write `[type:: MEET]` (the PT13 suggestion is not MEET).
    pub needs_type: bool,
    /// Whether this task row earns a MEET link (today, event unended).
    pub link_eligible: bool,
    pub all_day: bool,
    pub start_min: i32,
}

/// Pure planner input.
pub(crate) struct SyncPlanInput<'a> {
    pub contents: &'a str,
    pub is_today: bool,
    pub occurrences: Vec<PlannedOccurrence>,
    pub journal: &'a BTreeMap<(String, String), OccurrenceState>,
    pub day_string: String,
}

/// One row the plan inserts, for the journal and the report.
#[derive(Debug, Clone)]
pub(crate) struct PlannedRow {
    pub calendar_id: String,
    pub event_id: String,
    pub ical_uid: Option<String>,
    pub series_id: Option<String>,
    pub block_id: String,
    pub row_kind: String,
    pub spans: BTreeMap<String, String>,
    pub root: Option<String>,
    pub source: Option<String>,
    pub time_label: String,
    pub title: String,
}

/// One adopted crash-recovery row, for the journal and the report.
#[derive(Debug, Clone)]
pub(crate) struct AdoptedRow {
    pub calendar_id: String,
    pub event_id: String,
    pub ical_uid: Option<String>,
    pub series_id: Option<String>,
    pub block_id: String,
    pub row_kind: String,
    pub spans: BTreeMap<String, String>,
    pub root: Option<String>,
    pub source: Option<String>,
    pub link_adopted: bool,
    pub time_label: String,
    pub title: String,
}

/// One tombstoned journal row, for the journal and the report.
#[derive(Debug, Clone)]
pub(crate) struct GoneRow {
    pub calendar_id: String,
    pub event_id: String,
    pub block_id: String,
}

/// One MEET link the plan appends.
#[derive(Debug, Clone)]
pub(crate) struct PlannedLink {
    pub calendar_id: String,
    pub event_id: String,
    pub block_id: String,
}

/// The pure plan outcome.
#[derive(Debug, Clone, Default)]
pub(crate) struct SyncPlan {
    pub contents: String,
    pub changed: bool,
    pub rows_added: Vec<PlannedRow>,
    pub rows_adopted: Vec<AdoptedRow>,
    pub rows_gone: Vec<GoneRow>,
    pub links_added: Vec<PlannedLink>,
    pub pomodoro_entry_line: Option<usize>,
    pub pomodoro_created: bool,
}

pub(crate) fn plan_sync(input: &SyncPlanInput<'_>) -> SyncPlan {
    let mut working = WorkingNote::new(input.contents);
    let mut plan = SyncPlan {
        contents: input.contents.to_string(),
        ..SyncPlan::default()
    };
    let mut used: HashSet<String> = collect_block_ids(&working.lines);

    for occurrence in &input.occurrences {
        let key = occurrence_key(&occurrence.calendar_id, &occurrence.event_id);
        let journaled = input
            .journal
            .get(&(key, input.day_string.clone()))
            .and_then(|state| state.row.as_ref());
        // Context rows without a 📅 URL match by their full rendered
        // text, so URL-less events stay idempotent.
        let text_match = if !occurrence.task && occurrence.url().is_none() {
            Some(render_occurrence(occurrence, ""))
        } else {
            None
        };
        match journaled {
            Some(journaled) => {
                if find_row_line(
                    &working.lines,
                    working.events_range(),
                    &journaled.block_id,
                    occurrence.url(),
                    occurrence.task,
                    text_match.as_deref(),
                )
                .is_some()
                {
                    // Journaled and still present: nothing to do here.
                    // (Span updates arrive with gcal-lifecycle.)
                    continue;
                }
                plan.rows_gone.push(GoneRow {
                    calendar_id: occurrence.calendar_id.clone(),
                    event_id: occurrence.event_id.clone(),
                    block_id: journaled.block_id.clone(),
                });
            }
            None => {
                if let Some(line_index) = find_row_line(
                    &working.lines,
                    working.events_range(),
                    "",
                    occurrence.url(),
                    occurrence.task,
                    text_match.as_deref(),
                ) {
                    // Crash recovery: adopt the unjournaled line.
                    let block_id =
                        trailing_block_id(&working.lines[line_index]);
                    let link_adopted = !block_id.is_empty()
                        && pomodoro_link_present(&working.lines, &block_id);
                    if !block_id.is_empty() {
                        used.insert(block_id.clone());
                    }
                    plan.rows_adopted.push(AdoptedRow {
                        calendar_id: occurrence.calendar_id.clone(),
                        event_id: occurrence.event_id.clone(),
                        ical_uid: occurrence.ical_uid.clone(),
                        series_id: occurrence.series_id.clone(),
                        block_id,
                        row_kind: row_kind(occurrence.task),
                        spans: occurrence.spans.clone(),
                        root: occurrence.root.clone(),
                        source: occurrence.source.clone(),
                        link_adopted,
                        time_label: occurrence.time_label.clone(),
                        title: occurrence.title.clone(),
                    });
                    continue;
                }
                let block_id = if occurrence.task {
                    let minted =
                        crate::native::capture_block_ids::mint_block_id(
                            &occurrence.id_hint,
                            &used.iter().map(String::as_str).collect(),
                        );
                    used.insert(minted.clone());
                    minted
                } else {
                    String::new()
                };
                let text = render_occurrence(occurrence, &block_id);
                working.ensure_events_section();
                let slot = insert_slot(
                    &working,
                    occurrence.all_day,
                    occurrence.start_min,
                );
                working.insert_row(slot, &text);
                plan.rows_added.push(PlannedRow {
                    calendar_id: occurrence.calendar_id.clone(),
                    event_id: occurrence.event_id.clone(),
                    ical_uid: occurrence.ical_uid.clone(),
                    series_id: occurrence.series_id.clone(),
                    block_id,
                    row_kind: row_kind(occurrence.task),
                    spans: occurrence.spans.clone(),
                    root: occurrence.root.clone(),
                    source: occurrence.source.clone(),
                    time_label: occurrence.time_label.clone(),
                    title: occurrence.title.clone(),
                });
            }
        }
    }

    if input.is_today {
        plan_meet_links(input, &mut working, &mut plan);
    }

    plan.contents = working.finish();
    // A no-op plan writes nothing and appends nothing: identical
    // contents and no records. A tombstone-only run leaves the note
    // alone but still records `gcal.gone`, so it is not a no-op.
    let records_empty = plan.rows_added.is_empty()
        && plan.rows_adopted.is_empty()
        && plan.rows_gone.is_empty()
        && plan.links_added.is_empty();
    plan.changed = plan.contents != input.contents || !records_empty;
    if !plan.changed {
        plan.rows_added.clear();
        plan.rows_adopted.clear();
        plan.rows_gone.clear();
        plan.links_added.clear();
        plan.pomodoro_entry_line = None;
        plan.pomodoro_created = false;
    }
    plan
}

impl PlannedOccurrence {
    fn url(&self) -> Option<&str> {
        self.html_link.as_deref()
    }
}

fn row_kind(task: bool) -> String {
    if task {
        "task".to_string()
    } else {
        "context".to_string()
    }
}

fn render_occurrence(occurrence: &PlannedOccurrence, block_id: &str) -> String {
    if occurrence.task {
        render_task_row(
            &super::render::TaskRowInput {
                time_label: &occurrence.time_label,
                title: &occurrence.title,
                html_link: occurrence.html_link.as_deref(),
                join_link: occurrence.join_link.as_deref(),
                location: occurrence.location.as_deref(),
                root: occurrence.root.as_deref(),
                needs_type: occurrence.needs_type,
                linked: occurrence.link_eligible,
            },
            block_id,
        )
    } else {
        render_context_row(&ContextRowInput {
            time_label: &occurrence.time_label,
            title: &occurrence.title,
            html_link: occurrence.html_link.as_deref(),
            qualifier: occurrence.qualifier.as_deref(),
        })
    }
}

/// Owned working copy of the note: owned lines plus the input's
/// trailing-newline state, so an unchanged plan is byte-identical.
struct WorkingNote {
    lines: Vec<String>,
    trailing_newline: bool,
}

impl WorkingNote {
    fn new(contents: &str) -> Self {
        Self {
            lines: contents.lines().map(str::to_string).collect(),
            trailing_newline: contents.ends_with('\n'),
        }
    }

    fn replace(&mut self, contents: &str) {
        self.lines = contents.lines().map(str::to_string).collect();
        self.trailing_newline = contents.ends_with('\n');
    }

    fn as_string(&self) -> String {
        self.finish()
    }

    fn finish(&self) -> String {
        let mut out = self.lines.join("\n");
        if self.trailing_newline && !self.lines.is_empty() {
            out.push('\n');
        }
        out
    }

    fn line_refs(&self) -> Vec<&str> {
        self.lines.iter().map(String::as_str).collect()
    }

    fn events_range(&self) -> Option<Range<usize>> {
        let refs = self.line_refs();
        event_task::events_section_range(&refs)
    }

    /// Create `## Events` immediately above `## Pomodoros` with tidy
    /// blank lines. Callers guarantee the Pomodoros section exists.
    /// (Section ranges start at the body, so the heading sits one
    /// line above `pomodoros.start`.)
    fn ensure_events_section(&mut self) {
        if self.events_range().is_some() {
            return;
        }
        let refs = self.line_refs();
        let pomodoros = pomodoros_section_range(&refs).expect("Pomodoros");
        let mut insert_at = pomodoros.start.saturating_sub(1);
        while insert_at > 0 && self.lines[insert_at - 1].trim().is_empty() {
            insert_at -= 1;
        }
        let mut updated: Vec<String> = self.lines[..insert_at].to_vec();
        updated.push("## Events".to_string());
        updated.push(String::new());
        let mut tail: Vec<String> = self.lines[insert_at..].to_vec();
        while tail.first().is_some_and(|line| line.trim().is_empty()) {
            tail.remove(0);
        }
        updated.append(&mut tail);
        self.lines = updated;
    }

    fn insert_row(&mut self, slot: Option<usize>, text: &str) {
        match slot {
            Some(after) => self.lines.insert(after + 1, text.to_string()),
            None => {
                let start =
                    self.events_range().expect("Events section exists").start;
                self.lines.insert(start, text.to_string());
            }
        }
    }
}

fn collect_block_ids(lines: &[String]) -> HashSet<String> {
    let mut used = HashSet::new();
    for line in lines {
        if let Some(id) = parse_trailing_block_id(line) {
            used.insert(id);
        }
    }
    used
}

fn parse_trailing_block_id(line: &str) -> Option<String> {
    let trimmed = line.trim_end();
    let caret = trimmed.rfind('^')?;
    let id = trimmed.get(caret + 1..)?;
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || (caret > 0
            && !trimmed[..caret]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace))
    {
        return None;
    }
    Some(id.to_string())
}

fn trailing_block_id(line: &str) -> String {
    parse_trailing_block_id(line).unwrap_or_default()
}

/// Find a row line inside the Events section: a task matches its
/// journaled `^block_id` first, then its `📅` URL; a context row
/// matches its `📅` URL, or its full rendered text when it has none.
fn find_row_line(
    lines: &[String],
    events_range: Option<Range<usize>>,
    block_id: &str,
    url: Option<&str>,
    task: bool,
    text_match: Option<&str>,
) -> Option<usize> {
    let range = events_range?;
    if task && !block_id.is_empty() {
        let suffix = format!("^{block_id}");
        for index in range.clone() {
            let line = lines.get(index)?.trim_end();
            if line.strip_suffix(suffix.as_str()).is_some_and(|before| {
                before.chars().last().is_some_and(char::is_whitespace)
            }) {
                return Some(index);
            }
        }
    }
    if let Some(url) = url.filter(|url| !url.is_empty()) {
        for index in range.clone() {
            if lines.get(index).is_some_and(|line| line.contains(url)) {
                return Some(index);
            }
        }
    }
    if let Some(text) = text_match.filter(|text| !text.is_empty()) {
        for index in range {
            if lines.get(index).is_some_and(|line| line.trim_end() == text) {
                return Some(index);
            }
        }
    }
    None
}

fn pomodoro_link_present(lines: &[String], block_id: &str) -> bool {
    let link = format!("[[#^{block_id}]]");
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let range = match pomodoros_section_range(&refs) {
        Some(range) => range,
        None => return false,
    };
    range
        .filter_map(|index| refs.get(index))
        .any(|line| line.contains(link.as_str()))
}

/// Time-ordered slot: the 0-based line to insert after, or `None` for
/// the top of the section. After the last parsable row that starts at
/// or before the new row; all-day rows sort before timed rows.
fn insert_slot(
    working: &WorkingNote,
    all_day: bool,
    start_min: i32,
) -> Option<usize> {
    let range = working.events_range().expect("Events section exists");
    let key = (!all_day, start_min);
    let current = working.as_string();
    let spans = line_spans(&current);
    let mut slot: Option<usize> = None;
    for index in range {
        let line = &working.lines[index];
        if line.trim().is_empty() {
            continue;
        }
        let Some(row_key) = row_sort_key(line) else {
            continue;
        };
        if row_key <= key {
            slot = Some(child_block_end_line(&spans, index));
        } else {
            break;
        }
    }
    slot
}

/// Sort key for one Events row: all-day rows sort before timed rows
/// (the inverted flag sorts `false` first), then by start minute.
/// Unparsable lines are skipped by the caller.
fn row_sort_key(line: &str) -> Option<(bool, i32)> {
    if let Some(span) = event_task::parse_event_span(line) {
        return Some((true, i32::from(span.start_min)));
    }
    let body = line.trim_start_matches([' ', '\t']).strip_prefix("- ")?;
    let body = body.strip_prefix("**")?;
    let raw = body.get(..body.find("**")?)?;
    if raw == "all day" {
        return Some((false, 0));
    }
    if raw.len() == 9 && raw.as_bytes().get(4) == Some(&b'-') {
        let start = parse_hhmm(&raw[..4])?;
        return Some((true, i32::from(start)));
    }
    None
}

fn parse_hhmm(value: &str) -> Option<u16> {
    if value.len() != 4 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let hour = value[..2].parse::<u16>().ok()?;
    let minute = value[2..].parse::<u16>().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some(hour * 60 + minute)
}

/// Append each eligible MEET link once, creating the placeholder in
/// the same write as its first link.
fn plan_meet_links(
    input: &SyncPlanInput<'_>,
    working: &mut WorkingNote,
    plan: &mut SyncPlan,
) {
    // Eligible links: task rows present in the note (journaled, new,
    // or adopted) whose event has not ended, with no journaled link
    // and no tombstone. A removed link is never re-added.
    let mut wanted: Vec<(String, String, String)> = Vec::new();
    for occurrence in &input.occurrences {
        if !occurrence.task || !occurrence.link_eligible {
            continue;
        }
        let key = occurrence_key(&occurrence.calendar_id, &occurrence.event_id);
        if let Some(state) =
            input.journal.get(&(key.clone(), input.day_string.clone()))
            && (state.link || state.gone)
        {
            continue;
        }
        let Some(block_id) =
            journaled_or_planned_block_id(input, plan, occurrence)
                .filter(|id| !id.is_empty())
        else {
            continue;
        };
        if wanted.iter().any(|(_, _, id)| *id == block_id) {
            continue;
        }
        let refs = working.line_refs();
        if find_row_line(
            &working.lines,
            event_task::events_section_range(&refs),
            &block_id,
            occurrence.url(),
            true,
            None,
        )
        .is_none()
        {
            continue;
        }
        wanted.push((
            occurrence.calendar_id.clone(),
            occurrence.event_id.clone(),
            block_id,
        ));
    }
    if wanted.is_empty() {
        return;
    }
    let mut contents = working.as_string();
    let mut entry_line: Option<usize> = find_meet_entry(&contents);
    let mut created = false;
    for (calendar_id, event_id, block_id) in &wanted {
        let link = format!("[[#^{block_id}]]");
        if entry_line.is_none() {
            let (updated, created_line, _) =
                insert_named_placeholder_after_last_open(&contents, "MEET")
                    .expect("Pomodoros section exists");
            contents = updated;
            entry_line = Some(created_line);
            created = true;
        }
        let spans = line_spans(&contents);
        let (updated, already, _) = insert_link_into_entry(
            &contents,
            &spans,
            entry_line.expect("MEET entry"),
            &link,
        );
        if !already {
            contents = updated;
            plan.links_added.push(PlannedLink {
                calendar_id: calendar_id.clone(),
                event_id: event_id.clone(),
                block_id: block_id.clone(),
            });
        }
    }
    if !plan.links_added.is_empty() {
        working.replace(&contents);
        // `entry_line` is 0-based; the report is 1-based.
        plan.pomodoro_entry_line = entry_line.map(|line| line + 1);
        plan.pomodoro_created = created;
    }
}

fn journaled_or_planned_block_id(
    input: &SyncPlanInput<'_>,
    plan: &SyncPlan,
    occurrence: &PlannedOccurrence,
) -> Option<String> {
    let key = occurrence_key(&occurrence.calendar_id, &occurrence.event_id);
    if let Some(state) = input.journal.get(&(key, input.day_string.clone()))
        && let Some(row) = state.row.as_ref()
    {
        return Some(row.block_id.clone());
    }
    if let Some(added) = plan.rows_added.iter().find(|row| {
        row.calendar_id == occurrence.calendar_id
            && row.event_id == occurrence.event_id
    }) {
        return Some(added.block_id.clone());
    }
    plan.rows_adopted
        .iter()
        .find(|row| {
            row.calendar_id == occurrence.calendar_id
                && row.event_id == occurrence.event_id
        })
        .map(|row| row.block_id.clone())
}

/// The open MEET entry: running timed first, else any open MEET.
/// Returns the 0-based line index.
fn find_meet_entry(contents: &str) -> Option<usize> {
    let scan = capture_pomodoros::scan(contents);
    let mut fallback = None;
    for entry in &scan.entries {
        if entry.state != PomodoroState::Open {
            continue;
        }
        if entry.name.as_deref() != Some("MEET") {
            continue;
        }
        if entry.time_range.is_some() {
            return Some(entry.line - 1);
        }
        fallback = Some(entry.line - 1);
    }
    fallback
}

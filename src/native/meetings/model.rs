//! Meeting rows, window collection, and the filed-notes scan.
//!
//! A [`MeetingRow`] is one Event Task row in a canonical daily note,
//! enriched with its vault root, its Google identity from the gcal
//! journal, and the exact EVENT token that names it. [`filed_notes_index`]
//! scans `ref/meetings/*.md` frontmatter so list and agenda views can
//! show which meetings already have notes.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use chrono::{Duration, NaiveDate, NaiveDateTime};

use crate::native::{
    env as bob_env,
    event_task::{self, EventKind},
    gcal::{decode_calendar_eid, journal, snapshot::daily_relative_path},
    note_tasks,
    projects::{frontmatter_value, is_excluded_directory, parse_frontmatter},
    ref_tasks::find_managed_embed,
    task_complete::successors::is_inbox_note,
    task_root::{parse_task_root, TaskRootState},
    task_status_hooks::canonical_daily_date,
    vault_links::NoteIndex,
};

/// One Event Task row that can take meeting notes.
///
/// Shared with the `ref meet` CLI phase: several fields are only read
/// there and by tests so far.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MeetingRow {
    /// Vault-relative daily note, for example `2026/20261123.md`.
    pub path: PathBuf,
    /// One-based line number in the note.
    pub line: usize,
    /// The row's `^id`, if it has one. Rows without an id refuse filing
    /// with a hint to add one.
    pub block_id: Option<String>,
    /// The meeting's local day.
    pub date: NaiveDate,
    /// The meeting span as written, for example `1500-1530`.
    pub time_range: String,
    /// Local start and end datetimes (ends roll past midnight overnight).
    pub starts_at: NaiveDateTime,
    pub ends_at: NaiveDateTime,
    /// The clean event title.
    pub title: String,
    /// The checkbox status symbol (`' '`, `'*'`, `'x'`, `'-'`, …).
    pub status: char,
    /// The row's display root, if it names one.
    pub root: Option<String>,
    /// Whether that root is an inbox note (including `meet_inbox`).
    pub root_is_inbox: bool,
    /// The row's 📅 URL, when it carries one.
    pub calendar_url: Option<String>,
    /// Google identity from the journal or the URL, when known.
    pub event_id: Option<String>,
    pub series_id: Option<String>,
    pub calendar_id: Option<String>,
}

/// Notes already filed for a meeting.
///
/// Shared with the `ref meet` CLI phase: `status` is only read there and
/// by tests so far.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FiledNotes {
    /// Vault-relative reference note, for example
    /// `ref/meetings/261123_acme_corp_qbr.md`.
    pub note: String,
    /// The reading task as `<note>#^<id>`, for example
    /// `job#^ref-261123-acme-corp-qbr`.
    pub task: String,
    /// The reading task's checkbox status symbol, or `""` when unknown.
    pub status: String,
}

/// Filed notes keyed two ways: by Google event id when known, else by the
/// resolved `(note, block id)` target of the `event` frontmatter link.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FiledNotesIndex {
    pub by_event_id: BTreeMap<String, FiledNotes>,
    pub by_target: BTreeMap<(PathBuf, String), FiledNotes>,
}

/// Google identity folded from the journal for one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GoogleIdentity {
    calendar_id: String,
    event_id: String,
    series_id: Option<String>,
    url: Option<String>,
}

/// Local today, honoring `BOB_NOW` like every other gcal command.
///
/// Shared with the `ref meet` CLI phase; only window scans call it.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn local_today() -> NaiveDate {
    bob_env::current_datetime().date()
}

/// Every `.md` file under the vault as a link index (dot directories such
/// as `.bob` and `.git` stay out).
pub(crate) fn vault_index(vault: &Path) -> NoteIndex {
    NoteIndex::from_paths(list_markdown_files(vault))
}

fn list_markdown_files(vault: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![vault.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') || is_excluded_directory(&path) {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if path.extension().is_some_and(|ext| ext == "md")
                && let Ok(relative) = path.strip_prefix(vault)
            {
                files.push(relative.to_path_buf());
            }
        }
    }
    files.sort();
    files
}

/// The exact EVENT token for a row, for example
/// `20261123#^bryan-bugyi-derrick`. `None` without a block id.
pub(crate) fn event_ref_for(relative: &Path, block_id: &str) -> Option<String> {
    let stem = relative.file_stem()?.to_str()?;
    (!stem.is_empty() && !block_id.is_empty())
        .then(|| format!("{stem}#^{block_id}"))
}

/// Every Event Task row on the days in scope: one local day, or the last
/// `window_days` days ending today. Unreadable days are skipped.
///
/// Shared with the `ref meet` CLI phase; resolution and listing call it.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn collect_window_rows(
    vault: &Path,
    window_days: u32,
    day: Option<NaiveDate>,
) -> Vec<MeetingRow> {
    let today = local_today();
    let days: Vec<NaiveDate> = match day {
        Some(day) => vec![day],
        None => (0..window_days)
            .map(|back| today - Duration::days(i64::from(back)))
            .collect(),
    };
    let index = vault_index(vault);
    let (by_row, by_url) = journal_identities(vault);
    let mut rows = Vec::new();
    for day in days {
        let relative = daily_relative_path(day);
        if canonical_daily_date(&relative) != Some(day) {
            continue;
        }
        let Ok(contents) = fs::read_to_string(vault.join(&relative)) else {
            continue;
        };
        let lines: Vec<&str> = contents.lines().collect();
        for event in event_task::events_in_note(&contents, day)
            .into_iter()
            .filter(|event| event.kind == EventKind::Task)
        {
            let raw = lines.get(event.line_index).copied().unwrap_or("");
            rows.push(build_row(
                vault, &index, &relative, &event, raw, &by_row, &by_url,
            ));
        }
    }
    rows.sort_by(|left: &MeetingRow, right: &MeetingRow| {
        (left.date, left.starts_at, left.line).cmp(&(
            right.date,
            right.starts_at,
            right.line,
        ))
    });
    rows
}

/// Fold the journal into row and URL identity maps. The earliest day wins
/// a repeated `(path, block id)`; without a journal both maps are empty.
pub(crate) fn journal_identities(
    vault: &Path,
) -> (
    BTreeMap<(String, String), GoogleIdentity>,
    BTreeMap<String, GoogleIdentity>,
) {
    let mut by_row = BTreeMap::new();
    let mut by_url = BTreeMap::new();
    let Ok((state, _, _)) = journal::load_state(vault) else {
        return (by_row, by_url);
    };
    for ((key, _day), occurrence) in &state {
        let Some(row) = occurrence.row.as_ref() else {
            continue;
        };
        let Some((calendar_id, event_id)) = key.split_once('\u{1f}') else {
            continue;
        };
        let identity = GoogleIdentity {
            calendar_id: calendar_id.to_string(),
            event_id: event_id.to_string(),
            series_id: row.series_id.clone(),
            url: row.url.clone(),
        };
        by_row
            .entry((row.path.clone(), row.block_id.clone()))
            .or_insert_with(|| identity.clone());
        if let Some(url) = row.url.as_ref() {
            by_url.entry(url.clone()).or_insert(identity);
        }
    }
    (by_row, by_url)
}

/// Build one row from its parsed event, raw line, and journal maps.
///
/// The journal and root lookups rerun per row so explicit resolution
/// needs no window scan.
pub(crate) fn build_row(
    vault: &Path,
    index: &NoteIndex,
    relative: &Path,
    event: &event_task::EventNoteRow,
    raw: &str,
    by_row: &BTreeMap<(String, String), GoogleIdentity>,
    by_url: &BTreeMap<String, GoogleIdentity>,
) -> MeetingRow {
    let date = canonical_daily_date(relative)
        .expect("collect_window_rows only visits canonical days");
    let span = event.span.expect("task rows always carry a span");
    let midnight = date.and_hms_opt(0, 0, 0).expect("midnight is valid");
    let starts_at = midnight + Duration::minutes(i64::from(span.start_min));
    let ends_at =
        midnight + Duration::minutes(i64::from(span.end_on_timeline()));
    let calendar_url = calendar_url_in_line(raw);
    let identity = event
        .block_id
        .as_ref()
        .and_then(|id| {
            by_row.get(&(relative.to_string_lossy().into_owned(), id.clone()))
        })
        .or_else(|| calendar_url.as_ref().and_then(|url| by_url.get(url)));
    let (event_id, series_id, calendar_id) = match identity {
        Some(identity) => (
            Some(identity.event_id.clone()),
            identity.series_id.clone(),
            Some(identity.calendar_id.clone()),
        ),
        None => (
            calendar_url.as_deref().and_then(decode_calendar_eid),
            None,
            None,
        ),
    };
    let root = match parse_task_root(raw) {
        TaskRootState::Set { raw } => Some(display_root(&raw)),
        TaskRootState::None | TaskRootState::Repair { .. } => None,
    };
    let root_is_inbox = root
        .as_deref()
        .is_some_and(|root| root_is_inbox_note(vault, index, root));
    MeetingRow {
        path: relative.to_path_buf(),
        line: event.line_index + 1,
        block_id: event.block_id.clone(),
        date,
        time_range: span.text(),
        starts_at,
        ends_at,
        title: event.title.clone(),
        status: event.status.unwrap_or(' '),
        root,
        root_is_inbox,
        calendar_url,
        event_id,
        series_id,
        calendar_id,
    }
}

/// The 📅 badge target on one Events row.
fn calendar_url_in_line(line: &str) -> Option<String> {
    let marker = "[📅](";
    let start = line.find(marker)? + marker.len();
    let link = line.get(start..)?.split_once(')')?.0.trim();
    let target = link.split_once(" \"").map_or(link, |(target, _)| target);
    (!target.is_empty()).then(|| target.to_string())
}

/// The display form of a `[root::]` value: brackets and aliases stripped.
fn display_root(raw: &str) -> String {
    let raw = raw.trim();
    let raw = raw
        .strip_prefix("[[")
        .and_then(|value| value.strip_suffix("]]"))
        .unwrap_or(raw);
    raw.split_once('|')
        .map_or(raw, |(target, _)| target)
        .trim()
        .to_string()
}

/// Whether a root string names an inbox note (including `meet_inbox`).
fn root_is_inbox_note(vault: &Path, index: &NoteIndex, root: &str) -> bool {
    let Some(path) = index
        .resolve_detailed(None, root)
        .path()
        .map(Path::to_path_buf)
    else {
        return false;
    };
    let Ok(contents) = fs::read_to_string(vault.join(&path)) else {
        return false;
    };
    is_inbox_note(&path, &contents, index)
}

/// Resolve an EVENT note spelling to its vault-relative path.
///
/// Shared with the `ref meet` CLI phase.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn resolve_note_path(
    index: &NoteIndex,
    note: &str,
) -> Result<PathBuf, String> {
    match index.resolve_detailed(None, note) {
        crate::native::vault_links::LinkResolution::Found(path) => Ok(path),
        crate::native::vault_links::LinkResolution::Ambiguous => {
            Err(format!("`{note}` matches more than one note"))
        }
        crate::native::vault_links::LinkResolution::Missing => {
            Err(format!("no note matches `{note}`"))
        }
    }
}

/// Scan `ref/meetings/*.md` frontmatter (`event_id`, else the `event`
/// link) into the filed-notes index. The reading task comes from the
/// note's managed embed; its live status is read from the parent note.
pub(crate) fn filed_notes_index(vault: &Path) -> FiledNotesIndex {
    let mut index = FiledNotesIndex::default();
    let dir = vault.join("ref").join("meetings");
    let Ok(entries) = fs::read_dir(&dir) else {
        return index;
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    paths.sort();
    let link_index = vault_index(vault);
    let settings = note_tasks::read_settings(vault);
    for path in paths {
        let Ok(relative) = path.strip_prefix(vault).map(Path::to_path_buf)
        else {
            continue;
        };
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        let Some(frontmatter) = parse_frontmatter(&contents) else {
            continue;
        };
        let event_id = frontmatter_value(&frontmatter, "event_id")
            .map(|value| value.trim().trim_matches('"').to_string())
            .filter(|value| !value.is_empty());
        let target = frontmatter_value(&frontmatter, "event")
            .and_then(parse_event_link)
            .and_then(|(note, id)| {
                link_index
                    .resolve_detailed(None, &note)
                    .path()
                    .map(|path| (path.to_path_buf(), id))
            });
        if event_id.is_none() && target.is_none() {
            continue;
        }
        let filed = filed_notes_for(
            &link_index,
            vault,
            &settings,
            &relative,
            &contents,
        );
        if let Some(event_id) = event_id {
            index
                .by_event_id
                .entry(event_id)
                .or_insert_with(|| filed.clone());
        }
        if let Some(target) = target {
            index.by_target.entry(target).or_insert(filed);
        }
    }
    index
}

/// The filed-notes index for one vault, or `None` when no meeting notes
/// exist yet (every row then reports missing without a scan).
pub(crate) fn filed_index_for(vault: &Path) -> Option<FiledNotesIndex> {
    if !vault.join("ref").join("meetings").is_dir() {
        return None;
    }
    Some(filed_notes_index(vault))
}

/// Parse an `event` frontmatter wikilink into `(note, block id)`.
pub(crate) fn parse_event_link(value: &str) -> Option<(String, String)> {
    let value = value.trim().trim_matches('"').trim();
    let inner = value
        .strip_prefix("[[")
        .and_then(|rest| rest.find("]]").map(|end| &rest[..end]))?;
    let target = inner
        .split_once('|')
        .map_or(inner.trim(), |(head, _)| head.trim());
    let (note, id) = target.split_once("#^")?;
    let (note, id) = (note.trim(), id.trim());
    (!note.is_empty() && !id.is_empty())
        .then(|| (note.to_string(), id.to_string()))
}

/// Read one reference note's filed state from its managed embed and the
/// parent note's live task status.
fn filed_notes_for(
    link_index: &NoteIndex,
    vault: &Path,
    settings: &note_tasks::NoteTaskSettings,
    relative: &Path,
    contents: &str,
) -> FiledNotes {
    let note = relative.to_string_lossy().into_owned();
    let body = match parse_frontmatter(contents) {
        Some(frontmatter) => contents
            .lines()
            .skip(frontmatter.body_start_line)
            .collect::<Vec<_>>()
            .join("\n"),
        None => contents.to_string(),
    };
    let Some(embed) = find_managed_embed(&body) else {
        return FiledNotes {
            note,
            task: String::new(),
            status: String::new(),
        };
    };
    let parent = link_index
        .resolve_detailed(None, &embed.target)
        .path()
        .map(Path::to_path_buf);
    let Some(parent) = parent else {
        return FiledNotes {
            note,
            task: format!("{}#^{}", embed.target, embed.block_id),
            status: String::new(),
        };
    };
    let parent_name = parent.with_extension("").to_string_lossy().into_owned();
    let task = format!("{parent_name}#^{}", embed.block_id);
    let status = fs::read_to_string(vault.join(&parent))
        .ok()
        .and_then(|parent_contents| {
            match note_tasks::scan(&parent_contents, settings)
                .by_block_id(&embed.block_id)
            {
                note_tasks::BlockIdLookup::Found(task) => {
                    Some(task.status_symbol.to_string())
                }
                _ => None,
            }
        })
        .unwrap_or_default();
    FiledNotes { note, task, status }
}

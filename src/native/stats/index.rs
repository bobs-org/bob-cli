use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, NaiveDateTime};
use serde_yaml::Value as YamlValue;

use super::error::StatsError;
use crate::native::{
    capture_pomodoros, capture_pomodoros_agenda, collect_done, dataview,
    pomodoro_homes, projects, vault_links::NoteIndex,
};

/// Shared vault and journal facts for evaluators. The inputs phase
/// fills this; no evaluator performs I/O.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StatsIndex {
    pub(crate) tasks: Vec<IndexedTask>,
    pub(crate) daily_notes: BTreeMap<NaiveDate, DailyNoteInputs>,
    pub(crate) parent_of: BTreeMap<String, String>,
    pub(crate) journal_events: Vec<IndexedJournalEvent>,
    pub(crate) note_index: crate::native::vault_links::NoteIndex,
    /// Markdown buffers from the single vault pass, used to build the
    /// indexed homes source without a second read.
    pub(crate) buffers: Vec<(String, String)>,
    pub(crate) health: IndexHealth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexedTask {
    pub(crate) source_path: String,
    pub(crate) archive_path: Option<String>,
    pub(crate) status_type: String,
    pub(crate) created: DatedValue,
    pub(crate) done: DatedValue,
    pub(crate) cancelled: DatedValue,
    pub(crate) scheduled: DatedValue,
    pub(crate) recurring: bool,
    pub(crate) tags: Vec<String>,
    pub(crate) block_id: Option<String>,
    pub(crate) description: String,
    pub(crate) task_kind: TaskKindPart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskKindPart {
    Task,
    Prj,
    Ref,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DatedValue {
    pub(crate) raw: Option<String>,
    pub(crate) date: Option<NaiveDate>,
    pub(crate) diagnostic: DateDiagnostic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum DateDiagnostic {
    #[default]
    Missing,
    Ok,
    Malformed,
    Future,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DailyNoteInputs {
    pub(crate) path: String,
    pub(crate) has_pomodoros_heading: bool,
    pub(crate) closed_pomodoros: Vec<PomodoroSession>,
    pub(crate) frontmatter_fields: BTreeMap<String, String>,
    pub(crate) inline_fields: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct PomodoroSession {
    pub(crate) name: Option<String>,
    pub(crate) recorded_minutes: Option<u32>,
    pub(crate) range_minutes: Option<u32>,
    pub(crate) line: u32,
    pub(crate) links: Vec<pomodoro_homes::SessionLink>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexedJournalEvent {
    pub(crate) day: NaiveDate,
    pub(crate) ts: String,
    pub(crate) host: String,
    pub(crate) tier: String,
    pub(crate) outcome: String,
    pub(crate) key: String,
    pub(crate) path: String,
    pub(crate) line: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct IndexHealth {
    pub(crate) invalid_dates: u32,
    pub(crate) closed_without_date: u32,
    pub(crate) missing_created: u32,
    pub(crate) journal: JournalHealth,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct JournalHealth {
    pub(crate) since: Option<NaiveDate>,
    pub(crate) malformed_lines: u32,
    pub(crate) hosts: Vec<JournalHostHealth>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JournalHostHealth {
    pub(crate) host: String,
    pub(crate) events: u32,
    pub(crate) last_ts: String,
    pub(crate) last_day: NaiveDate,
}

impl StatsIndex {
    /// Read and parse the vault once. Markdown content buffers are shared by
    /// task, daily-note, and parent-link indexing; evaluators never need I/O.
    pub(crate) fn read(
        vault: &Path,
        now: NaiveDateTime,
    ) -> Result<Self, StatsError> {
        Self::read_with(vault, now, |path| fs::read_to_string(path))
    }

    fn read_with<F>(
        vault: &Path,
        now: NaiveDateTime,
        reader: F,
    ) -> Result<Self, StatsError>
    where
        F: Fn(&Path) -> io::Result<String> + Sync,
    {
        // Probe the vault directory first so a missing or unreadable vault
        // surfaces as vault I/O instead of masquerading as a format refusal
        // when the default (non-Dataview) settings kick in.
        fs::read_dir(vault).map_err(|error| {
            StatsError::io(format!("read {}: {error}", vault.display()))
        })?;
        let parser = dataview::StatsTaskParser::read_settings(vault, now)
            .map_err(stats_settings_error)?;
        if !parser.is_dataview() {
            return Err(StatsError::non_dataview());
        }

        let mut paths = Vec::new();
        collect_markdown_paths(vault, vault, &mut paths)?;
        paths.sort();
        let buffers = read_markdown_parallel(vault, &paths, &reader)?;

        let relative_paths = buffers
            .iter()
            .map(|(path, _)| PathBuf::from(path))
            .collect::<Vec<_>>();
        let note_index = NoteIndex::from_paths(relative_paths);
        let mut index = Self::default();
        let mut parent_candidates = Vec::new();

        for (relative, contents) in &buffers {
            let task_rows: Vec<dataview::StatsTaskRecord> =
                parser.parse_file(contents, relative.clone());
            for task in task_rows {
                let actual_path = task.path;
                let archive_source =
                    collect_done::source_relative_path_from_archive(Path::new(
                        &actual_path,
                    ))
                    .map(|path| path_string(&path));
                let source_path = archive_source
                    .clone()
                    .unwrap_or_else(|| actual_path.clone());
                let created = dated_value(task.created, now.date());
                let done = dated_value(task.done, now.date());
                let cancelled = dated_value(task.cancelled, now.date());
                let scheduled = dated_value(task.scheduled, now.date());

                if date_counts_as_invalid(
                    &created, &done, &cancelled, &scheduled,
                ) {
                    index.health.invalid_dates += 1;
                }
                if task.status_type == "DONE" && done.raw.is_none()
                    || task.status_type == "CANCELLED"
                        && cancelled.raw.is_none()
                {
                    index.health.closed_without_date += 1;
                }
                if created.raw.is_none() {
                    index.health.missing_created += 1;
                }

                let task_kind = if task
                    .block_id
                    .as_deref()
                    .is_some_and(|id| id.eq_ignore_ascii_case("prj"))
                {
                    TaskKindPart::Prj
                } else if task
                    .block_id
                    .as_deref()
                    .is_some_and(|id| id.eq_ignore_ascii_case("ref"))
                    || task.tags.iter().any(|tag| {
                        tag.trim_start_matches('#').eq_ignore_ascii_case("ref")
                    })
                {
                    TaskKindPart::Ref
                } else {
                    TaskKindPart::Task
                };

                index.tasks.push(IndexedTask {
                    source_path,
                    archive_path: archive_source.map(|_| actual_path),
                    status_type: task.status_type,
                    created,
                    done,
                    cancelled,
                    scheduled,
                    recurring: task.recurring,
                    tags: task.tags,
                    block_id: task.block_id,
                    description: task.description,
                    task_kind,
                });
            }

            if let Some(date) = canonical_daily_date(relative) {
                index
                    .daily_notes
                    .insert(date, parse_daily_note(relative, contents));
            }

            if let Some(target) = parent_target(contents)
                && let Some(parent) =
                    note_index.resolve(Some(Path::new(relative)), &target)
            {
                let actual_child = Path::new(relative);
                let source_child =
                    collect_done::source_relative_path_from_archive(
                        actual_child,
                    )
                    .unwrap_or_else(|| actual_child.to_path_buf());
                let source_parent =
                    collect_done::source_relative_path_from_archive(&parent)
                        .unwrap_or(parent);
                let is_archive = source_child.as_path() != actual_child;
                parent_candidates.push((
                    is_archive,
                    path_string(&source_child),
                    path_string(&source_parent),
                    relative.clone(),
                ));
            }
        }

        parent_candidates.sort();
        for (_, child, parent, _) in parent_candidates {
            if child != parent {
                index.parent_of.entry(child).or_insert(parent);
            }
        }
        index.note_index = note_index;
        index.buffers = buffers;
        index.journal_events = read_journal(vault, &mut index.health.journal)?;
        Ok(index)
    }

    /// Return all transitive parent paths, stopping safely at missing links
    /// and cycles. The input note itself is never repeated in the result.
    pub(crate) fn parent_ancestors(&self, path: &str) -> Vec<String> {
        let mut seen = std::collections::BTreeSet::from([path.to_string()]);
        let mut ancestors = Vec::new();
        let mut current = path;
        while let Some(parent) = self.parent_of.get(current) {
            if !seen.insert(parent.clone()) {
                break;
            }
            ancestors.push(parent.clone());
            current = parent;
        }
        ancestors
    }
}

fn stats_settings_error(error: dataview::DataviewError) -> StatsError {
    // Match `bob ready` (`freshness/scan.rs`): settings read and parse
    // failures are vault I/O (exit 1), with Display-formatted messages.
    match error {
        dataview::DataviewError::TasksSettingsRead { path, error } => {
            StatsError::io(format!(
                "read Tasks settings {}: {error}",
                path.display()
            ))
        }
        dataview::DataviewError::TasksSettingsParse { path, error } => {
            StatsError::io(format!(
                "parse Tasks settings {}: {error}",
                path.display()
            ))
        }
        other => StatsError::io(format!("read Tasks settings: {other}")),
    }
}

fn collect_markdown_paths(
    vault: &Path,
    directory: &Path,
    paths: &mut Vec<PathBuf>,
) -> Result<(), StatsError> {
    let entries = fs::read_dir(directory).map_err(|error| {
        StatsError::io(format!("read {}: {error}", directory.display()))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            StatsError::io(format!(
                "read directory {}: {error}",
                directory.display()
            ))
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            StatsError::io(format!("inspect {}: {error}", path.display()))
        })?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let name = entry.file_name();
            if excluded_directory(&name) {
                continue;
            }
            collect_markdown_paths(vault, &path, paths)?;
        } else if file_type.is_file() && is_markdown(&path) {
            let relative = path.strip_prefix(vault).map_err(|error| {
                StatsError::io(format!(
                    "make {} vault-relative: {error}",
                    path.display()
                ))
            })?;
            paths.push(relative.to_path_buf());
        }
    }
    Ok(())
}

fn excluded_directory(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    name.starts_with('.')
        || matches!(name, "_conflicts" | "_generated" | "_templates")
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

fn read_markdown_parallel<F>(
    vault: &Path,
    relative_paths: &[PathBuf],
    reader: &F,
) -> Result<Vec<(String, String)>, StatsError>
where
    F: Fn(&Path) -> io::Result<String> + Sync,
{
    if relative_paths.is_empty() {
        return Ok(Vec::new());
    }
    let worker_count = std::thread::available_parallelism()
        .map(|parallelism| parallelism.get())
        .unwrap_or(4)
        .min(relative_paths.len())
        .max(1);
    let next = AtomicUsize::new(0);
    let chunks = std::thread::scope(|scope| {
        let handles = (0..worker_count)
            .map(|_| {
                scope.spawn(|| {
                    let mut local = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(relative) = relative_paths.get(index) else {
                            break;
                        };
                        let full_path = vault.join(relative);
                        local.push((index, reader(&full_path)));
                    }
                    local
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("stats input worker panicked"))
            .collect::<Vec<_>>()
    });
    let mut results = chunks.into_iter().flatten().collect::<Vec<_>>();
    results.sort_by_key(|(index, _)| *index);
    results
        .into_iter()
        .map(|(index, contents)| {
            let relative = &relative_paths[index];
            let contents = contents.map_err(|error| {
                StatsError::io(format!(
                    "read {}: {error}",
                    vault.join(relative).display()
                ))
            })?;
            Ok((path_string(relative), contents))
        })
        .collect()
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn date_counts_as_invalid(
    created: &DatedValue,
    done: &DatedValue,
    cancelled: &DatedValue,
    scheduled: &DatedValue,
) -> bool {
    let event_invalid = |date: &DatedValue| {
        matches!(
            date.diagnostic,
            DateDiagnostic::Malformed | DateDiagnostic::Future
        )
    };
    event_invalid(created)
        || event_invalid(done)
        || event_invalid(cancelled)
        || scheduled.diagnostic == DateDiagnostic::Malformed
}

fn dated_value(raw: Option<String>, today: NaiveDate) -> DatedValue {
    let Some(raw) = raw else {
        return DatedValue::default();
    };
    let date = NaiveDate::parse_from_str(&raw, "%Y-%m-%d").ok();
    let diagnostic = match date {
        None => DateDiagnostic::Malformed,
        Some(date) if date > today => DateDiagnostic::Future,
        Some(_) => DateDiagnostic::Ok,
    };
    DatedValue {
        raw: Some(raw),
        date,
        diagnostic,
    }
}

pub(crate) fn canonical_daily_date(relative: &str) -> Option<NaiveDate> {
    let mut components = relative.split('/');
    let year = components.next()?;
    let filename = components.next()?;
    if components.next().is_some()
        || year.len() != 4
        || !year.bytes().all(|byte| byte.is_ascii_digit())
        || filename.len() != 11
        || !filename.ends_with(".md")
    {
        return None;
    }
    let digits = &filename[..8];
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let expected_year = year.parse::<i32>().ok()?;
    let date = NaiveDate::parse_from_str(digits, "%Y%m%d").ok()?;
    (date.year() == expected_year).then_some(date)
}

fn parse_daily_note(relative: &str, contents: &str) -> DailyNoteInputs {
    let (frontmatter, body_start) = split_frontmatter(contents);
    let frontmatter_fields = parse_frontmatter_fields(frontmatter);
    let body_lines = contents.lines().skip(body_start).collect::<Vec<_>>();
    let inline_fields = parse_inline_fields(&body_lines);
    let scan = capture_pomodoros::scan(contents);
    let lines = contents.lines().collect::<Vec<_>>();
    let closed_pomodoros = scan
        .entries
        .iter()
        .filter(|entry| {
            entry.state == capture_pomodoros::PomodoroState::Completed
        })
        .map(|entry| {
            let raw_line = entry
                .line
                .checked_sub(1)
                .and_then(|line| lines.get(line).copied())
                .unwrap_or("");
            let recorded =
                crate::native::capture::duration_from_range_text(raw_line)
                    .and_then(|minutes| u32::try_from(minutes).ok());
            let range = entry
                .time_range
                .as_deref()
                .and_then(capture_pomodoros_agenda::range_minutes)
                .and_then(|minutes| u32::try_from(minutes).ok());
            PomodoroSession {
                name: entry.name.clone(),
                recorded_minutes: recorded,
                range_minutes: range,
                line: u32::try_from(entry.line).unwrap_or(u32::MAX),
                links: pomodoro_homes::session_links(
                    contents,
                    entry.line.saturating_sub(1),
                ),
            }
        })
        .collect();
    DailyNoteInputs {
        path: relative.to_string(),
        has_pomodoros_heading: scan.has_section,
        closed_pomodoros,
        frontmatter_fields,
        inline_fields,
    }
}

fn split_frontmatter(contents: &str) -> (Option<&str>, usize) {
    let mut lines = contents.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return (None, 0);
    };
    if first.trim_end_matches(['\r', '\n']) != "---" {
        return (None, 0);
    }
    let mut offset = first.len();
    let mut body_start = 1;
    for line in lines {
        body_start += 1;
        if line.trim_end_matches(['\r', '\n']) == "---" {
            return (Some(&contents[first.len()..offset]), body_start);
        }
        offset += line.len();
    }
    (None, 0)
}

fn parse_frontmatter_fields(
    frontmatter: Option<&str>,
) -> BTreeMap<String, String> {
    let Some(frontmatter) = frontmatter else {
        return BTreeMap::new();
    };
    let Ok(YamlValue::Mapping(mapping)) =
        serde_yaml::from_str::<YamlValue>(frontmatter)
    else {
        return BTreeMap::new();
    };
    mapping
        .into_iter()
        .filter_map(|(key, value)| {
            let key = key.as_str()?.to_string();
            let value = yaml_scalar_text(&value)?;
            Some((key, value))
        })
        .collect()
}

fn yaml_scalar_text(value: &YamlValue) -> Option<String> {
    match value {
        YamlValue::String(value) => Some(value.clone()),
        YamlValue::Null => None,
        YamlValue::Number(value) => Some(value.to_string()),
        YamlValue::Bool(value) => Some(value.to_string()),
        YamlValue::Sequence(_) | YamlValue::Mapping(_) => {
            serde_yaml::to_string(value)
                .ok()
                .map(|value| value.trim().to_string())
        }
        _ => None,
    }
}

fn parse_inline_fields(lines: &[&str]) -> BTreeMap<String, String> {
    static BRACKET_FIELD: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| {
            regex::Regex::new(r"\[([A-Za-z][A-Za-z0-9_-]*)::\s*([^\]]*)\]")
                .expect("valid bracket inline field regex")
        });
    static PLAIN_FIELD: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| {
            regex::Regex::new(r"^\s*([A-Za-z][A-Za-z0-9_-]*)::\s*(.*?)\s*$")
                .expect("valid plain inline field regex")
        });

    let mut fields = BTreeMap::new();
    let mut fence: Option<(char, usize)> = None;
    let mut in_comment = false;
    for line in lines {
        let line = without_html_comments(line, &mut in_comment);
        if line.trim().is_empty() {
            continue;
        }
        if update_inline_fence(&line, &mut fence) || fence.is_some() {
            continue;
        }
        let trimmed = line.trim_start();
        if is_list_or_quote_line(trimmed) {
            continue;
        }
        for captures in BRACKET_FIELD.captures_iter(&line) {
            if let (Some(key), Some(value)) = (captures.get(1), captures.get(2))
            {
                fields.insert(
                    key.as_str().to_string(),
                    value.as_str().trim().to_string(),
                );
            }
        }
        if let Some(captures) = PLAIN_FIELD.captures(&line)
            && let (Some(key), Some(value)) = (captures.get(1), captures.get(2))
        {
            fields.insert(
                key.as_str().to_string(),
                value.as_str().trim().to_string(),
            );
        }
    }
    fields
}

fn is_list_or_quote_line(trimmed: &str) -> bool {
    let Some(first) = trimmed.chars().next() else {
        return false;
    };
    if matches!(first, '-' | '*' | '+' | '>') {
        return true;
    }
    if !first.is_ascii_digit() {
        return false;
    }
    let after_digits =
        trimmed.trim_start_matches(|ch: char| ch.is_ascii_digit());
    after_digits.starts_with('.')
}

fn without_html_comments(line: &str, in_comment: &mut bool) -> String {
    let mut output = String::new();
    let mut rest = line;
    loop {
        if *in_comment {
            if let Some(end) = rest.find("-->") {
                rest = &rest[end + 3..];
                *in_comment = false;
            } else {
                break;
            }
        } else if let Some(start) = rest.find("<!--") {
            output.push_str(&rest[..start]);
            rest = &rest[start + 4..];
            *in_comment = true;
        } else {
            output.push_str(rest);
            break;
        }
    }
    output
}

fn update_inline_fence(line: &str, fence: &mut Option<(char, usize)>) -> bool {
    let trimmed = line.trim_start();
    let Some(marker @ ('`' | '~')) = trimmed.chars().next() else {
        return false;
    };
    let count = trimmed.chars().take_while(|value| *value == marker).count();
    if count < 3 {
        return false;
    }
    match *fence {
        Some((open, width)) if marker == open && count >= width => {
            *fence = None
        }
        None => *fence = Some((marker, count)),
        _ => {}
    }
    true
}

fn parent_target(contents: &str) -> Option<String> {
    let (frontmatter, _) = split_frontmatter(contents);
    let frontmatter = frontmatter?;
    let raw = frontmatter.lines().find_map(|line| {
        let line = line.trim_start();
        let rest = line.strip_prefix("parent:")?;
        Some(rest.trim())
    })?;
    let raw = projects::trim_yaml_scalar(raw);
    let inner = raw
        .strip_prefix("[[")
        .and_then(|value| value.strip_suffix("]]"))
        .unwrap_or(raw);
    let before_alias =
        inner.split_once('|').map_or(inner, |(target, _)| target);
    let target = before_alias
        .split_once('#')
        .map_or(before_alias, |(target, _)| target)
        .trim();
    (!target.is_empty()).then(|| target.to_string())
}

fn read_journal(
    vault: &Path,
    health: &mut JournalHealth,
) -> Result<Vec<IndexedJournalEvent>, StatsError> {
    let paths = collect_journal_paths(vault)?;
    let mut events = Vec::new();
    let mut hosts = BTreeMap::<String, HostAccumulator>::new();
    for path in paths {
        let bytes = fs::read(&path).map_err(|error| {
            StatsError::io(format!("read {}: {error}", path.display()))
        })?;
        let relative = path
            .strip_prefix(vault)
            .map(path_string)
            .unwrap_or_else(|_| path_string(&path));
        for (line_index, line) in
            journal_file_lines(&bytes).into_iter().enumerate()
        {
            let Ok(line) = line else {
                health.malformed_lines += 1;
                continue;
            };
            if line.trim().is_empty() {
                continue;
            }
            let Some((event, timestamp)) = parse_journal_line(
                &line,
                &relative,
                u32::try_from(line_index + 1).unwrap_or(u32::MAX),
            ) else {
                health.malformed_lines += 1;
                continue;
            };
            health.since = Some(
                health.since.map_or(event.day, |since| since.min(event.day)),
            );
            let host = event.host.clone();
            let entry = hosts.entry(host).or_default();
            entry.events += 1;
            let is_latest = entry.latest.as_ref().is_none_or(|latest| {
                timestamp > latest.timestamp
                    || timestamp == latest.timestamp
                        && (&event.path, event.line)
                            > (&latest.path, latest.line)
            });
            if is_latest {
                entry.latest = Some(HostLatest {
                    timestamp,
                    ts: event.ts.clone(),
                    day: event.day,
                    path: event.path.clone(),
                    line: event.line,
                });
            }
            events.push(event);
        }
    }
    health.hosts = hosts
        .into_iter()
        .filter_map(|(host, entry)| {
            let latest = entry.latest?;
            Some(JournalHostHealth {
                host,
                events: entry.events,
                last_ts: latest.ts,
                last_day: latest.day,
            })
        })
        .collect();
    events.sort_by(|left, right| {
        left.day
            .cmp(&right.day)
            .then_with(|| {
                DateTime::parse_from_rfc3339(&left.ts)
                    .expect("validated timestamp")
                    .cmp(
                        &DateTime::parse_from_rfc3339(&right.ts)
                            .expect("validated timestamp"),
                    )
            })
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.line.cmp(&right.line))
            .then_with(|| left.host.cmp(&right.host))
    });
    Ok(events)
}

#[derive(Default)]
struct HostAccumulator {
    events: u32,
    latest: Option<HostLatest>,
}

struct HostLatest {
    timestamp: DateTime<FixedOffset>,
    ts: String,
    day: NaiveDate,
    path: String,
    line: u32,
}

fn collect_journal_paths(vault: &Path) -> Result<Vec<PathBuf>, StatsError> {
    let directory = vault.join(".bob/stats/review");
    let mut current = vault.to_path_buf();
    for component in [".bob", "stats", "review"] {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => return Ok(Vec::new()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Vec::new());
            }
            Err(error) => {
                return Err(StatsError::io(format!(
                    "inspect {}: {error}",
                    current.display()
                )));
            }
        }
    }
    let entries = fs::read_dir(&directory).map_err(|error| {
        StatsError::io(format!("read {}: {error}", directory.display()))
    })?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            StatsError::io(format!(
                "read directory {}: {error}",
                directory.display()
            ))
        })?;
        if entry
            .file_type()
            .map_err(|error| {
                StatsError::io(format!(
                    "inspect {}: {error}",
                    entry.path().display()
                ))
            })?
            .is_file()
            && entry.path().extension() == Some(OsStr::new("jsonl"))
        {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

fn journal_file_lines(bytes: &[u8]) -> Vec<Result<String, ()>> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        let mut end = index;
        if end > start && bytes[end - 1] == b'\r' {
            end -= 1;
        }
        lines.push(
            std::str::from_utf8(&bytes[start..end])
                .map(str::to_string)
                .map_err(|_| ()),
        );
        start = index + 1;
    }
    if start < bytes.len() {
        let mut end = bytes.len();
        if end > start && bytes[end - 1] == b'\r' {
            end -= 1;
        }
        lines.push(
            std::str::from_utf8(&bytes[start..end])
                .map(str::to_string)
                .map_err(|_| ()),
        );
    }
    lines
}

fn parse_journal_line(
    line: &str,
    path: &str,
    line_number: u32,
) -> Option<(IndexedJournalEvent, DateTime<FixedOffset>)> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if value.get("v")?.as_u64()? != 1
        || value.get("kind")?.as_str()? != "review.answer"
    {
        return None;
    }
    let ts = required_json_string(&value, "ts")?;
    let timestamp = DateTime::parse_from_rfc3339(ts).ok()?;
    let day_raw = required_json_string(&value, "day")?;
    if !is_iso_day(day_raw) {
        return None;
    }
    let day = NaiveDate::parse_from_str(day_raw, "%Y-%m-%d").ok()?;
    let host = required_json_string(&value, "host")?.to_string();
    let tier = required_json_string(&value, "tier")?.to_string();
    let outcome = required_json_string(&value, "outcome")?.to_string();
    let key = required_json_string(&value, "key")?.to_string();
    Some((
        IndexedJournalEvent {
            day,
            ts: ts.to_string(),
            host,
            tier,
            outcome,
            key,
            path: path.to_string(),
            line: line_number,
        },
        timestamp,
    ))
}

fn required_json_string<'a>(
    value: &'a serde_json::Value,
    key: &str,
) -> Option<&'a str> {
    value
        .get(key)?
        .as_str()
        .filter(|value| !value.trim().is_empty())
}

fn is_iso_day(value: &str) -> bool {
    value.len() == 10
        && value.bytes().enumerate().all(|(index, byte)| match index {
            4 | 7 => byte == b'-',
            _ => byte.is_ascii_digit(),
        })
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fs, io,
        path::Path,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use chrono::{NaiveDate, NaiveDateTime};
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn default_index_is_empty() {
        let index = StatsIndex::default();
        assert!(index.tasks.is_empty());
        assert!(index.daily_notes.is_empty());
        assert!(index.parent_of.is_empty());
        assert!(index.journal_events.is_empty());
        assert_eq!(index.health.invalid_dates, 0);
        assert_eq!(index.health.closed_without_date, 0);
        assert_eq!(index.health.missing_created, 0);
        assert!(index.health.journal.hosts.is_empty());
    }

    #[test]
    fn interface_rows_hold_residence_dates_and_journal_facts() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 11).unwrap();
        let task = IndexedTask {
            source_path: "sase.md".to_string(),
            archive_path: Some("done/sase_done.md".to_string()),
            status_type: "DONE".to_string(),
            created: DatedValue {
                raw: Some("2026-10-01".to_string()),
                date: Some(day),
                diagnostic: DateDiagnostic::Ok,
            },
            done: DatedValue {
                raw: Some("bad".to_string()),
                date: None,
                diagnostic: DateDiagnostic::Malformed,
            },
            cancelled: DatedValue {
                raw: None,
                date: None,
                diagnostic: DateDiagnostic::Missing,
            },
            scheduled: DatedValue {
                raw: Some("2099-01-01".to_string()),
                date: NaiveDate::from_ymd_opt(2099, 1, 1),
                diagnostic: DateDiagnostic::Future,
            },
            recurring: false,
            tags: vec!["ref".to_string()],
            block_id: Some("ref".to_string()),
            description: "read".to_string(),
            task_kind: TaskKindPart::Ref,
        };
        assert_eq!(task.task_kind, TaskKindPart::Ref);
        assert_eq!(TaskKindPart::Prj, TaskKindPart::Prj);
        assert_eq!(TaskKindPart::Task, TaskKindPart::Task);
        let daily = DailyNoteInputs {
            path: "2026/20261011.md".to_string(),
            has_pomodoros_heading: true,
            closed_pomodoros: vec![PomodoroSession {
                name: Some("GTD".to_string()),
                recorded_minutes: Some(25),
                range_minutes: Some(30),
                line: 12,
                links: Vec::new(),
            }],
            frontmatter_fields: BTreeMap::from([(
                "energy".to_string(),
                "3".to_string(),
            )]),
            inline_fields: BTreeMap::new(),
        };
        assert_eq!(daily.closed_pomodoros[0].recorded_minutes, Some(25));
        let event = IndexedJournalEvent {
            day,
            ts: "2026-10-11T05:41:07-04:00".to_string(),
            host: "apollo".to_string(),
            tier: "next".to_string(),
            outcome: "keep".to_string(),
            key: "sase.md#x".to_string(),
            path: ".bob/stats/review/2026-10.apollo.jsonl".to_string(),
            line: 1,
        };
        assert_eq!(event.outcome, "keep");
        let host = JournalHostHealth {
            host: "apollo".to_string(),
            events: 41,
            last_ts: event.ts.clone(),
            last_day: day,
        };
        assert_eq!(host.events, 41);
    }

    fn fixed_now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, 11)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
    }

    fn settings(vault: &Path, format: &str) {
        let path =
            vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            format!(
                r##"{{
                  "taskFormat":"{format}",
                  "globalFilter":"#task",
                  "statusSettings":{{
                    "customStatuses":[
                      {{"symbol":"*","name":"Custom done","type":"DONE"}},
                      {{"symbol":"~","name":"Not a task","type":"NON_TASK"}},
                      {{"symbol":"!","name":"Canceled","type":"CANCELLED"}}
                    ]
                  }}
                }}"##
            ),
        )
        .unwrap();
    }

    fn write_note(vault: &Path, relative: &str, contents: &str) {
        let path = vault.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn reads_each_markdown_file_once_and_restores_path_order() {
        let vault = TempDir::new().unwrap();
        settings(vault.path(), "dataview");
        write_note(vault.path(), "z.md", "- [ ] #task last\n");
        write_note(vault.path(), "a.md", "- [ ] #task first\n");
        let reads = AtomicUsize::new(0);
        let index = StatsIndex::read_with(vault.path(), fixed_now(), |path| {
            reads.fetch_add(1, Ordering::SeqCst);
            fs::read_to_string(path)
        })
        .unwrap();

        assert_eq!(reads.load(Ordering::SeqCst), 2);
        assert_eq!(
            index
                .tasks
                .iter()
                .map(|task| task.source_path.as_str())
                .collect::<Vec<_>>(),
            ["a.md", "z.md"]
        );
    }

    #[test]
    fn task_parser_honors_custom_done_non_task_fences_and_date_diagnostics() {
        let vault = TempDir::new().unwrap();
        settings(vault.path(), "dataview");
        write_note(
            vault.path(),
            "tasks.md",
            concat!(
                "- [*] #task Custom done [created:: 2026-10-01] ",
                "[completion:: 2026-10-10] #ref ^ref\n",
                "- [x] #task Bad date [created:: 2026-02-30] ^prj\n",
                "- [!] #task Closed without cancelled date\n",
                "- [~] #task Non task status\n",
                "<!--\n- [x] #task Commented out\n-->\n",
                "```tasks\n- [x] #task Fenced out\n```\n",
                "- [ ] #task Future [created:: 2026-10-12]\n",
            ),
        );
        let index = StatsIndex::read(vault.path(), fixed_now()).unwrap();

        let custom_done = index
            .tasks
            .iter()
            .find(|task| task.description.contains("Custom done"))
            .unwrap();
        assert_eq!(custom_done.status_type, "DONE");
        assert_eq!(custom_done.task_kind, TaskKindPart::Ref);
        let project = index
            .tasks
            .iter()
            .find(|task| task.description.contains("Bad date"))
            .unwrap();
        assert_eq!(project.task_kind, TaskKindPart::Prj);
        assert_eq!(project.created.diagnostic, DateDiagnostic::Malformed);
        let future = index
            .tasks
            .iter()
            .find(|task| task.description.contains("Future"))
            .unwrap();
        assert_eq!(future.created.diagnostic, DateDiagnostic::Future);
        assert_eq!(index.tasks.len(), 5);
        let non_task = index
            .tasks
            .iter()
            .find(|task| task.description.contains("Non task"))
            .unwrap();
        assert_eq!(non_task.status_type, "NON_TASK");
        // Future created counts; future scheduled on a different task does
        // not. The malformed created date is the other invalid date.
        assert_eq!(index.health.invalid_dates, 2);
        assert_eq!(index.health.closed_without_date, 2);
        assert_eq!(index.health.missing_created, 2);
    }

    #[test]
    fn future_scheduled_dates_are_not_invalid() {
        let vault = TempDir::new().unwrap();
        settings(vault.path(), "dataview");
        write_note(
            vault.path(),
            "tasks.md",
            concat!(
                "- [ ] #task Deferred [created:: 2026-10-10] ",
                "[scheduled:: 2026-10-20]\n",
                "- [ ] #task Bad schedule [created:: 2026-10-10] ",
                "[scheduled:: 2026-13-01]\n",
            ),
        );
        let index = StatsIndex::read(vault.path(), fixed_now()).unwrap();
        assert_eq!(index.health.invalid_dates, 1);
        let deferred = index
            .tasks
            .iter()
            .find(|task| task.description.contains("Deferred"))
            .unwrap();
        assert_eq!(deferred.scheduled.diagnostic, DateDiagnostic::Future);
    }

    #[test]
    fn canonical_daily_notes_include_ledgers_and_ignore_lookalikes() {
        let vault = TempDir::new().unwrap();
        settings(vault.path(), "dataview");
        write_note(
            vault.path(),
            "2026/20261011.md",
            concat!(
                "---\nenergy: 3\n---\n",
                "## Pomodoros\n",
                "- [x] (**08:00 - 08:30** [t:: 25m]) — GTD + BOB\n",
                "- [X] (0900-0930) — GTD\n",
                "- [ ] (1000-1030) — OPEN\n",
                "energy:: 4\n",
                "<!-- hidden:: 99 -->\n",
                "```text\nignored:: 88\n```\n",
            ),
        );
        write_note(vault.path(), "notes/20261011.md", "energy:: 90\n");
        write_note(vault.path(), "2026/20261032.md", "energy:: 90\n");
        write_note(vault.path(), "2025/20261011.md", "energy:: 90\n");

        let index = StatsIndex::read(vault.path(), fixed_now()).unwrap();
        let day = NaiveDate::from_ymd_opt(2026, 10, 11).unwrap();
        let note = index.daily_notes.get(&day).unwrap();
        assert_eq!(note.path, "2026/20261011.md");
        assert!(note.has_pomodoros_heading);
        assert_eq!(note.closed_pomodoros.len(), 2);
        assert_eq!(note.closed_pomodoros[0].recorded_minutes, Some(25));
        assert_eq!(note.closed_pomodoros[0].range_minutes, Some(30));
        assert_eq!(note.closed_pomodoros[0].name.as_deref(), Some("GTD + BOB"));
        assert_eq!(note.closed_pomodoros[1].recorded_minutes, None);
        assert_eq!(
            note.inline_fields.get("energy").map(String::as_str),
            Some("4")
        );
        assert!(!note.inline_fields.contains_key("hidden"));
        assert!(!note.inline_fields.contains_key("ignored"));
        assert_eq!(index.daily_notes.len(), 1);
    }

    #[test]
    fn archive_residence_and_parent_chains_resolve_with_cycle_and_ambiguity_guards(
    ) {
        let vault = TempDir::new().unwrap();
        settings(vault.path(), "dataview");
        write_note(
            vault.path(),
            "Project.md",
            "---\nparent: \"[[Area]]\"\n---\n",
        );
        write_note(
            vault.path(),
            "Area.md",
            "---\nparent: \"[[Project]]\"\n---\n",
        );
        write_note(
            vault.path(),
            "child/Source.md",
            "---\nparent: \"[[Project]]\"\n---\n",
        );
        write_note(
            vault.path(),
            "done/child/Source_done.md",
            "---\nparent: \"[[Project]]\"\n---\n- [x] #task Read ^ref\n",
        );
        write_note(vault.path(), "a/shared.md", "# A\n");
        write_note(vault.path(), "b/shared.md", "# B\n");
        write_note(
            vault.path(),
            "ambiguous.md",
            "---\nparent: \"[[shared]]\"\n---\n",
        );

        let index = StatsIndex::read(vault.path(), fixed_now()).unwrap();
        let archived = index
            .tasks
            .iter()
            .find(|task| task.archive_path.is_some())
            .unwrap();
        assert_eq!(archived.source_path, "child/Source.md");
        assert_eq!(
            archived.archive_path.as_deref(),
            Some("done/child/Source_done.md")
        );
        assert_eq!(archived.task_kind, TaskKindPart::Ref);
        assert_eq!(
            index.parent_of.get("child/Source.md").map(String::as_str),
            Some("Project.md")
        );
        assert_eq!(index.parent_ancestors("Project.md"), ["Area.md"]);
        assert_eq!(index.parent_ancestors("Area.md"), ["Project.md"]);
        assert!(!index.parent_of.contains_key("ambiguous.md"));
    }

    #[test]
    fn journal_reader_validates_lines_and_orders_offsets_and_ties_deterministically(
    ) {
        let vault = TempDir::new().unwrap();
        settings(vault.path(), "dataview");
        let directory = vault.path().join(".bob/stats/review");
        fs::create_dir_all(&directory).unwrap();
        let valid = |host: &str, day: &str, ts: &str, outcome: &str| {
            format!(
                r#"{{"v":1,"ts":"{ts}","day":"{day}","host":"{host}","kind":"review.answer","tier":"next","outcome":"{outcome}","key":"task#x"}}"#
            )
        };
        let tie = "2026-10-11T05:00:00-04:00";
        fs::write(
            directory.join("2026-10.apollo.jsonl"),
            format!(
                "{}\n{}\n{{\"v\":2}}\n\n{}\n{{truncated",
                valid(
                    "apollo",
                    "2026-10-11",
                    "2026-10-11T09:00:00+00:00",
                    "keep"
                ),
                valid("apollo", "2026-10-11", tie, "done"),
                valid(
                    "apollo",
                    "2026-10-09",
                    "2026-10-11T04:59:00-04:00",
                    "drop"
                )
            ),
        )
        .unwrap();
        fs::write(
            directory.join("2026-10.apollo.z.jsonl"),
            format!("{}\n", valid("apollo", "2026-10-11", tie, "defer")),
        )
        .unwrap();
        fs::write(
            directory.join("2026-10.mac.jsonl"),
            format!(
                "{}\n",
                valid(
                    "mac",
                    "2026-10-10",
                    "2026-10-10T10:00:00-04:00",
                    "today"
                )
            ),
        )
        .unwrap();

        let index = StatsIndex::read(vault.path(), fixed_now()).unwrap();
        assert_eq!(index.health.journal.malformed_lines, 2);
        assert_eq!(
            index.health.journal.since,
            NaiveDate::from_ymd_opt(2026, 10, 9)
        );
        assert_eq!(index.health.journal.hosts.len(), 2);
        assert_eq!(index.health.journal.hosts[0].host, "apollo");
        assert_eq!(index.health.journal.hosts[0].events, 4);
        assert_eq!(index.health.journal.hosts[0].last_ts, tie);
        assert_eq!(
            index.health.journal.hosts[0].last_day,
            NaiveDate::from_ymd_opt(2026, 10, 11).unwrap()
        );
        assert_eq!(
            index.journal_events.last().unwrap().path,
            ".bob/stats/review/2026-10.apollo.z.jsonl"
        );
    }

    #[test]
    fn settings_parse_and_task_format_fail_with_stats_errors() {
        let missing_format = TempDir::new().unwrap();
        assert!(matches!(
            StatsIndex::read(missing_format.path(), fixed_now()),
            Err(StatsError::Format { .. })
        ));

        let malformed = TempDir::new().unwrap();
        let settings_path = malformed
            .path()
            .join(".obsidian/plugins/obsidian-tasks-plugin/data.json");
        fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
        fs::write(settings_path, "{").unwrap();
        let error = StatsIndex::read(malformed.path(), fixed_now())
            .expect_err("malformed Tasks settings is I/O like bob ready");
        assert!(matches!(error, StatsError::Io { .. }));
        assert!(
            error.message().contains("parse Tasks settings"),
            "settings error uses Display, got {}",
            error.message()
        );

        let unreadable = TempDir::new().unwrap();
        let settings_path = unreadable
            .path()
            .join(".obsidian/plugins/obsidian-tasks-plugin/data.json");
        fs::create_dir_all(settings_path).unwrap();
        assert!(matches!(
            StatsIndex::read(unreadable.path(), fixed_now()),
            Err(StatsError::Io { .. })
        ));
    }

    #[test]
    fn ref_tag_without_block_id_classifies_as_ref_kind() {
        let vault = TempDir::new().unwrap();
        settings(vault.path(), "dataview");
        write_note(
            vault.path(),
            "tasks.md",
            concat!(
                "- [x] #task Read paper #ref [created:: 2026-10-09] ",
                "[completion:: 2026-10-09]\n",
                "- [x] #task Plain chore [created:: 2026-10-09] ",
                "[completion:: 2026-10-09]\n",
            ),
        );
        let index = StatsIndex::read(vault.path(), fixed_now()).unwrap();
        let tagged = index
            .tasks
            .iter()
            .find(|task| task.description.contains("Read paper"))
            .unwrap();
        assert_eq!(tagged.task_kind, TaskKindPart::Ref);
        let plain = index
            .tasks
            .iter()
            .find(|task| task.description.contains("Plain chore"))
            .unwrap();
        assert_eq!(plain.task_kind, TaskKindPart::Task);
    }

    #[test]
    fn missing_vault_directory_fails_as_io() {
        let missing = TempDir::new().unwrap().path().join("no-such-vault");
        assert!(matches!(
            StatsIndex::read(&missing, fixed_now()),
            Err(StatsError::Io { .. })
        ));
    }

    #[test]
    fn invalid_utf8_journal_lines_are_malformed_not_io() {
        let vault = TempDir::new().unwrap();
        settings(vault.path(), "dataview");
        let directory = vault.path().join(".bob/stats/review");
        fs::create_dir_all(&directory).unwrap();
        let mut bytes = Vec::from(
            r#"{"v":1,"ts":"2026-10-11T09:00:00+00:00","day":"2026-10-11","host":"apollo","kind":"review.answer","tier":"next","outcome":"keep","key":"task#x"}"#,
        );
        bytes.push(b'\n');
        bytes.extend_from_slice(&[0xff, 0xfe, 0xfd]);
        fs::write(directory.join("2026-10.apollo.jsonl"), bytes).unwrap();
        let index = StatsIndex::read(vault.path(), fixed_now()).unwrap();
        assert_eq!(index.journal_events.len(), 1);
        assert_eq!(index.health.journal.malformed_lines, 1);
    }

    #[test]
    fn ordered_list_task_fields_do_not_leak_into_daily_fields() {
        let vault = TempDir::new().unwrap();
        settings(vault.path(), "dataview");
        write_note(
            vault.path(),
            "2026/20261011.md",
            concat!(
                "energy:: 4\n",
                "1. [ ] #task Ordered [created:: 2026-10-01]\n",
                "- [ ] #task Bullet [created:: 2026-10-02]\n",
            ),
        );
        let index = StatsIndex::read(vault.path(), fixed_now()).unwrap();
        let note = index
            .daily_notes
            .get(&NaiveDate::from_ymd_opt(2026, 10, 11).unwrap())
            .unwrap();
        assert_eq!(
            note.inline_fields.get("energy").map(String::as_str),
            Some("4")
        );
        assert!(!note.inline_fields.contains_key("created"));
    }

    #[test]
    fn yaml_null_frontmatter_fields_are_absent() {
        let vault = TempDir::new().unwrap();
        settings(vault.path(), "dataview");
        write_note(
            vault.path(),
            "2026/20261011.md",
            "---\nenergy:\nmood: ok\n---\nenergy:: 3\n",
        );
        let index = StatsIndex::read(vault.path(), fixed_now()).unwrap();
        let note = index
            .daily_notes
            .get(&NaiveDate::from_ymd_opt(2026, 10, 11).unwrap())
            .unwrap();
        assert!(!note.frontmatter_fields.contains_key("energy"));
        assert_eq!(
            note.frontmatter_fields.get("mood").map(String::as_str),
            Some("ok")
        );
        assert_eq!(
            note.inline_fields.get("energy").map(String::as_str),
            Some("3")
        );
    }

    #[test]
    fn excluded_directories_are_skipped_and_done_is_included() {
        let vault = TempDir::new().unwrap();
        settings(vault.path(), "dataview");
        write_note(
            vault.path(),
            ".obsidian/hidden.md",
            "- [ ] #task Hidden obsidian\n",
        );
        write_note(
            vault.path(),
            "_conflicts/conflict.md",
            "- [ ] #task Hidden conflict\n",
        );
        write_note(
            vault.path(),
            "_generated/gen.md",
            "- [ ] #task Hidden generated\n",
        );
        write_note(
            vault.path(),
            "_templates/template.md",
            "- [ ] #task Hidden template\n",
        );
        write_note(
            vault.path(),
            "done/tasks_done.md",
            "- [x] #task Archived [created:: 2026-10-09] [completion:: 2026-10-09]\n",
        );
        write_note(vault.path(), "tasks.md", "- [ ] #task Visible\n");
        let index = StatsIndex::read(vault.path(), fixed_now()).unwrap();
        let paths: Vec<_> = index
            .tasks
            .iter()
            .map(|task| task.source_path.as_str())
            .collect();
        assert!(paths.contains(&"tasks.md"));
        assert!(
            paths.contains(&"tasks.md")
                || paths.iter().any(|path| path.contains("done"))
        );
        assert!(index
            .tasks
            .iter()
            .any(|task| task.description.contains("Archived")));
        assert!(index
            .tasks
            .iter()
            .all(|task| { !task.description.contains("Hidden") }));
    }

    #[test]
    fn symlinked_directories_are_not_followed() {
        let vault = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        settings(vault.path(), "dataview");
        write_note(vault.path(), "real/note.md", "- [ ] #task Real\n");
        write_note(outside.path(), "hidden.md", "- [ ] #task Hidden link\n");
        std::os::unix::fs::symlink(outside.path(), vault.path().join("linked"))
            .unwrap();
        let index = StatsIndex::read(vault.path(), fixed_now()).unwrap();
        assert!(index
            .tasks
            .iter()
            .any(|task| task.description.contains("Real")));
        assert!(index
            .tasks
            .iter()
            .all(|task| !task.description.contains("Hidden link")));
    }

    #[test]
    fn unreadable_markdown_surfaces_io() {
        let vault = TempDir::new().unwrap();
        settings(vault.path(), "dataview");
        write_note(vault.path(), "tasks.md", "- [ ] #task Visible\n");
        let error = StatsIndex::read_with(vault.path(), fixed_now(), |path| {
            if path.ends_with("tasks.md") {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"))
            } else {
                fs::read_to_string(path)
            }
        })
        .expect_err("unreadable markdown is Io");
        assert!(matches!(error, StatsError::Io { .. }));
        assert!(error.message().contains("denied"));
    }
}

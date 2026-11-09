//! Route repair, alert-once problems, and host-local sync staleness.
//!
//! Inbox rows are recomputed every run. A resolving route moves the task
//! through the task-move engine with no freshness stamp; a persisting
//! problem updates the managed warning and reports `first_seen: false`;
//! leftover unrouted labels drop the warning. Rows outside the inbox never
//! move. A routing label that names a different note is reported once as
//! `gmail.route_conflict`.
//!
//! Live runs keep `$XDG_STATE_HOME/bob/gmail/sync-state.json` so a quiet
//! busy-skip can fail after two hours without a successful run.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::{
    scan::{WatchScan, WatchingTask},
    ThreadKey,
};
use crate::native::{
    collect_done::{leading_indent_len, split_line_ending, task_block_end},
    env as bob_env,
    gmail::{
        journal::NAMESPACE,
        route::{LabelsById, RouteContext, RouteProblem, RouteVerdict},
        save, GmailError,
    },
    pref_journal::{self, StoredEvent},
};

/// Quiet busy-skips fail once the last successful run is this old.
pub(crate) const STALE_AFTER: Duration = Duration::from_secs(2 * 60 * 60);

const REFILED_KIND: &str = "gmail.refiled";
const CONFLICT_KIND: &str = "gmail.route_conflict";
const ROUTE_WARNING_MARKER: &str = "⚠️ **GMAIL ROUTE:**";

static STATE_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Inputs the apply transaction needs to recompute routes under the lock.
#[derive(Debug, Clone)]
pub(crate) struct HealthConfig {
    pub(crate) labels_by_id: LabelsById,
    pub(crate) enrollment_label: Option<String>,
    pub(crate) route_prefixes: Vec<String>,
}

/// One health action for an enrolled watching task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HealthIntent {
    pub(crate) key: ThreadKey,
    pub(crate) task_path: String,
    pub(crate) task_line: usize,
    pub(crate) block_id: Option<String>,
    pub(crate) task_text: String,
    pub(crate) note: Option<String>,
    pub(crate) action: HealthAction,
}

/// What health does to one watching task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HealthAction {
    /// Move the inbox block to the resolved area or project note.
    Refile { dest_file: String, route: String },
    /// Write or replace the managed `GMAIL ROUTE` child.
    SetWarning { problem: RouteProblem },
    /// Drop the managed warning; the row stays in the inbox.
    ClearWarning,
    /// Report the same problem again without failing the run.
    PersistProblem { problem: RouteProblem },
    /// One-time conflict against a row that already lives outside the inbox.
    Conflict {
        dest_file: String,
        route: String,
        label: String,
    },
}

/// Body-free journal event appended after durable health writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HealthEvent {
    Refiled {
        key: ThreadKey,
        note: Option<String>,
        task_file: String,
        route: String,
    },
    RouteConflict {
        key: ThreadKey,
        note: Option<String>,
        task_file: String,
        label: String,
        dest_file: String,
    },
}

/// Host-local last-attempt / last-success timestamps plus the busy-skip streak.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct SyncState {
    #[serde(default)]
    pub(crate) last_attempt_at: Option<String>,
    #[serde(default)]
    pub(crate) last_success_at: Option<String>,
    /// RFC 3339 UTC of the first consecutive live busy-skip after the last
    /// success. Old files still parse via the default.
    #[serde(default)]
    pub(crate) skipping_since: Option<String>,
}

impl HealthEvent {
    fn kind(&self) -> &'static str {
        match self {
            Self::Refiled { .. } => REFILED_KIND,
            Self::RouteConflict { .. } => CONFLICT_KIND,
        }
    }

    fn key(&self) -> &ThreadKey {
        match self {
            Self::Refiled { key, .. } | Self::RouteConflict { key, .. } => key,
        }
    }

    fn value(&self) -> Value {
        let mut value = Map::new();
        let key = self.key();
        value.insert("account".into(), json!(key.account()));
        value.insert("thread_id".into(), json!(key.thread_id()));
        match self {
            Self::Refiled {
                note,
                task_file,
                route,
                ..
            } => {
                if let Some(note) = note {
                    value.insert("note".into(), json!(note));
                }
                value.insert("task_file".into(), json!(task_file));
                value.insert("route".into(), json!(route));
            }
            Self::RouteConflict {
                note,
                task_file,
                label,
                dest_file,
                ..
            } => {
                if let Some(note) = note {
                    value.insert("note".into(), json!(note));
                }
                value.insert("task_file".into(), json!(task_file));
                value.insert("label".into(), json!(label));
                value.insert("dest_file".into(), json!(dest_file));
            }
        }
        Value::Object(value)
    }
}

/// The managed first-child warning line for a route problem.
pub(crate) fn route_warning_child(problem: &RouteProblem) -> String {
    let mut line = format!(
        "\t- {ROUTE_WARNING_MARKER} `{}` {}",
        problem.label.replace('`', "\\`"),
        problem.message
    );
    if !problem.hint.is_empty() {
        line.push_str(" — ");
        line.push_str(&problem.hint);
    }
    line
}

/// `$XDG_STATE_HOME/bob/gmail/sync-state.json`.
pub(crate) fn state_path() -> PathBuf {
    bob_env::state_home().join("bob/gmail/sync-state.json")
}

/// Load host-local sync timestamps. A missing file is empty state.
pub(crate) fn load_state() -> Result<SyncState, String> {
    let path = state_path();
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(SyncState::default())
        }
        Err(error) => {
            return Err(format!("could not read {}: {error}", path.display()))
        }
    };
    if bytes.is_empty() {
        return Ok(SyncState::default());
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("could not parse {}: {error}", path.display()))
}

fn write_state(state: &SyncState) -> io::Result<()> {
    let path = state_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(state)
        .map_err(|error| io::Error::other(error.to_string()))?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let temp = parent.join(format!(
        ".sync-state-{}-{}.tmp",
        std::process::id(),
        STATE_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    if let Err(error) = fs::write(&temp, &bytes).and_then(|()| {
        fs::rename(&temp, &path).inspect_err(|_| {
            let _ = fs::remove_file(&temp);
        })
    }) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    Ok(())
}

fn now_rfc3339() -> String {
    save::current_utc().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Record that a live run attempted to sync. Dry-run must not call this.
pub(crate) fn record_attempt() -> Result<(), GmailError> {
    let mut state = load_state().unwrap_or_default();
    state.last_attempt_at = Some(now_rfc3339());
    write_state(&state).map_err(|error| {
        GmailError::runtime(
            "sync_state",
            format!("could not write Gmail sync state: {error}"),
        )
    })
}

/// Record that a live run completed (including first-seen route problems).
pub(crate) fn record_success() -> Result<(), GmailError> {
    let now = now_rfc3339();
    let mut state = load_state().unwrap_or_default();
    state.last_attempt_at = Some(now.clone());
    state.last_success_at = Some(now);
    state.skipping_since = None;
    write_state(&state).map_err(|error| {
        GmailError::runtime(
            "sync_state",
            format!("could not write Gmail sync state: {error}"),
        )
    })
}

/// When a quiet busy-skip should fail: `last_success_at` older than two hours.
/// Kept for the doctor `sync` row, which is unchanged by the streak rule.
pub(crate) fn stale_skip_message(busy_what: &str) -> Option<String> {
    let state = load_state().ok()?;
    let last = state.last_success_at.as_deref()?;
    let parsed = DateTime::parse_from_rfc3339(last).ok()?.with_timezone(&Utc);
    let age = save::current_utc()
        .signed_duration_since(parsed)
        .to_std()
        .unwrap_or_default();
    if age <= STALE_AFTER {
        return None;
    }
    Some(format!(
        "gmail sync skipped: last successful run was {} ({last}); {busy_what} \
         is busy. Wait for it to finish, then retry, or run `bob gmail doctor`.",
        ago_from(age)
    ))
}

/// Live busy-skip streak: record this skip and report whether it must fail.
///
/// - If `skipping_since` is absent or older than `last_success_at`, set it to
///   now and do not alarm (the streak starts after the last success).
/// - Otherwise, when `now - skipping_since > STALE_AFTER`, return the existing
///   actionable message, naming the last success or "never".
/// - `record_success` clears the streak; other failures leave it alone.
/// - Dry runs must not call this (they write no state).
pub(crate) fn note_busy_skip(
    busy_what: &str,
) -> Result<Option<String>, GmailError> {
    let now = save::current_utc();
    let now_text = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut state = load_state().unwrap_or_default();
    state.last_attempt_at = Some(now_text.clone());
    let success_ts = state
        .last_success_at
        .as_deref()
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .map(|dt| dt.with_timezone(&Utc));
    let skipping_ts = state
        .skipping_since
        .as_deref()
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .map(|dt| dt.with_timezone(&Utc));
    let restart = match (skipping_ts, success_ts) {
        (None, _) => true,
        (Some(skip), Some(success)) => skip <= success,
        (Some(_), None) => false,
    };
    if restart {
        state.skipping_since = Some(now_text);
        write_state(&state).map_err(|error| {
            GmailError::runtime(
                "sync_state",
                format!("could not write Gmail sync state: {error}"),
            )
        })?;
        return Ok(None);
    }
    let skip_ts =
        skipping_ts.expect("restart false means skipping_since parses");
    let age = now
        .signed_duration_since(skip_ts)
        .to_std()
        .unwrap_or_default();
    // Persist the attempt even when alarming.
    write_state(&state).map_err(|error| {
        GmailError::runtime(
            "sync_state",
            format!("could not write Gmail sync state: {error}"),
        )
    })?;
    if age <= STALE_AFTER {
        return Ok(None);
    }
    let last_text = state.last_success_at.as_deref().unwrap_or("never");
    let last_age = success_ts
        .map(|ts| now.signed_duration_since(ts).to_std().unwrap_or_default());
    let age_text = match last_age {
        Some(age) => ago_from(age),
        None => {
            let skip_age = now
                .signed_duration_since(skip_ts)
                .to_std()
                .unwrap_or_default();
            ago_from(skip_age)
        }
    };
    Ok(Some(format!(
        "gmail sync skipped: last successful run was {age_text} ({last_text}); {busy_what} \
         is busy. Wait for it to finish, then retry, or run `bob gmail doctor`.",
    )))
}

/// Relative age for the doctor `sync` row (`3 min ago`, `2 h ago`).
pub(crate) fn ago_label(ts: &str) -> Option<String> {
    let parsed = DateTime::parse_from_rfc3339(ts).ok()?.with_timezone(&Utc);
    let age = save::current_utc()
        .signed_duration_since(parsed)
        .to_std()
        .unwrap_or_default();
    Some(format!("{} ago", ago_from(age)))
}

fn ago_from(age: Duration) -> String {
    let seconds = age.as_secs();
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes} min");
    }
    let hours = minutes / 60;
    if hours < 48 {
        return format!("{hours} h");
    }
    format!("{} d", hours / 24)
}

/// Whether `path` is the configured sync inbox, slash-normalized.
pub(crate) fn is_inbox_path(path: &str, inbox: &str) -> bool {
    normalize_note_path(path) == normalize_note_path(inbox)
}

fn normalize_note_path(path: &str) -> String {
    save::path_to_slashes(Path::new(path.trim().trim_start_matches("./")))
}

/// Fold one-time `gmail.route_conflict` reports across every host journal.
pub(crate) fn load_conflicts(vault: &Path) -> BTreeSet<ThreadKey> {
    let (events, _) = pref_journal::read_raw(vault, NAMESPACE)
        .unwrap_or_else(|_| (Vec::new(), Vec::new()));
    events
        .iter()
        .filter(|event| event.kind == CONFLICT_KIND)
        .filter_map(parse_conflict_key)
        .collect()
}

fn parse_conflict_key(event: &StoredEvent) -> Option<ThreadKey> {
    let object = event.value.get("value")?.as_object()?;
    ThreadKey::new(
        object.get("account")?.as_str()?,
        object.get("thread_id")?.as_str()?,
    )
}

/// Append a health journal event after the vault writes it describes.
pub(crate) fn append_event(
    vault: &Path,
    host: &str,
    event: &HealthEvent,
) -> io::Result<()> {
    let ts = pref_journal::event_timestamp()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let mut line = serde_json::to_string(&json!({
        "v": 1,
        "ts": ts,
        "host": host,
        "kind": event.kind(),
        "value": event.value(),
    }))
    .map_err(io::Error::other)?;
    line.push('\n');
    pref_journal::append_line(vault, NAMESPACE, host, &line)
}

/// Recompute routes for enrolled watching tasks and emit health intents.
pub(crate) fn plan(
    vault: &Path,
    inbox: &str,
    enrollment: &super::enroll::EnrollmentFold,
    watchers: &WatchScan,
    label_ids: &BTreeMap<ThreadKey, BTreeSet<String>>,
    config: &HealthConfig,
    route_ctx: &RouteContext,
    already_conflicted: &BTreeSet<ThreadKey>,
) -> Vec<HealthIntent> {
    let mut intents = Vec::new();
    let mut notes: HashMap<String, String> = HashMap::new();
    for key in enrollment.threads() {
        let Some(ids) = label_ids.get(key) else {
            continue;
        };
        let verdict = crate::native::gmail::route::route(
            &config.labels_by_id,
            ids,
            config.enrollment_label.as_deref(),
            &config.route_prefixes,
            route_ctx,
        );
        for task in watchers.watchers(key) {
            if is_inbox_path(&task.path, inbox) {
                if let Some(intent) =
                    inbox_intent(vault, &mut notes, key, task, &verdict)
                {
                    intents.push(intent);
                }
            } else if let Some(intent) = conflict_intent(
                key,
                task,
                &verdict,
                already_conflicted,
                config,
                ids,
                route_ctx,
            ) {
                intents.push(intent);
            }
        }
    }
    intents
}

fn inbox_intent(
    vault: &Path,
    notes: &mut HashMap<String, String>,
    key: &ThreadKey,
    task: &WatchingTask,
    verdict: &RouteVerdict,
) -> Option<HealthIntent> {
    let contents = notes.entry(task.path.clone()).or_insert_with(|| {
        fs::read_to_string(vault.join(&task.path)).unwrap_or_default()
    });
    let current_warning = existing_warning(contents, task.line);
    let note = task.target_note(key).map(str::to_string);
    let action = match verdict {
        RouteVerdict::Route(parent) => HealthAction::Refile {
            dest_file: format!("{}.md", parent.route),
            route: parent.route.clone(),
        },
        RouteVerdict::Unrouted => {
            current_warning.as_ref()?;
            HealthAction::ClearWarning
        }
        RouteVerdict::Problem(problem) => {
            let wanted = route_warning_child(problem);
            if current_warning.as_deref() == Some(wanted.as_str()) {
                HealthAction::PersistProblem {
                    problem: problem.clone(),
                }
            } else {
                HealthAction::SetWarning {
                    problem: problem.clone(),
                }
            }
        }
    };
    Some(HealthIntent {
        key: key.clone(),
        task_path: task.path.clone(),
        task_line: task.line,
        block_id: task.block_id.clone(),
        task_text: task.text.clone(),
        note,
        action,
    })
}

fn conflict_intent(
    key: &ThreadKey,
    task: &WatchingTask,
    verdict: &RouteVerdict,
    already_conflicted: &BTreeSet<ThreadKey>,
    config: &HealthConfig,
    ids: &BTreeSet<String>,
    route_ctx: &RouteContext,
) -> Option<HealthIntent> {
    let RouteVerdict::Route(parent) = verdict else {
        return None;
    };
    let dest = format!("{}.md", parent.route);
    if normalize_note_path(&task.path) == normalize_note_path(&dest) {
        return None;
    }
    if already_conflicted.contains(key) {
        return None;
    }
    Some(HealthIntent {
        key: key.clone(),
        task_path: task.path.clone(),
        task_line: task.line,
        block_id: task.block_id.clone(),
        task_text: task.text.clone(),
        note: task.target_note(key).map(str::to_string),
        action: HealthAction::Conflict {
            dest_file: dest,
            route: parent.route.clone(),
            label: winning_label(config, ids, &parent.route, route_ctx),
        },
    })
}

/// The Gmail label name that resolved to `dest_route`, when it is still on
/// the thread: the actual prefixed label whose remainder resolves to the
/// route through the same `RouteContext` (stem or alias, case-insensitive).
pub(crate) fn winning_label(
    config: &HealthConfig,
    ids: &BTreeSet<String>,
    dest_route: &str,
    route_ctx: &RouteContext,
) -> String {
    for id in ids {
        let Some(label) = config.labels_by_id.get(id) else {
            continue;
        };
        let name = label.name.as_str();
        if label_resolves_to_route(
            name,
            &config.route_prefixes,
            route_ctx,
            dest_route,
        ) {
            return label.name.clone();
        }
    }
    // Case-insensitive stem fallback for labels that resolve outside prefix
    // mode or predate the context scan.
    for id in ids {
        let Some(label) = config.labels_by_id.get(id) else {
            continue;
        };
        let name = label.name.as_str();
        let stem = name.rsplit('/').next().unwrap_or(name);
        if stem.eq_ignore_ascii_case(dest_route) {
            return label.name.clone();
        }
    }
    dest_route.to_string()
}

fn label_resolves_to_route(
    name: &str,
    prefixes: &[String],
    route_ctx: &RouteContext,
    dest_route: &str,
) -> bool {
    if prefixes.is_empty() {
        return route_ctx
            .resolve_route(name)
            .is_some_and(|p| p.route == dest_route);
    }
    for prefix in prefixes {
        let Some(rest) = name
            .strip_prefix(prefix.as_str())
            .and_then(|s| s.strip_prefix('/'))
        else {
            continue;
        };
        if rest.contains('/') {
            continue;
        }
        if route_ctx
            .resolve_route(rest)
            .is_some_and(|p| p.route == dest_route)
        {
            return true;
        }
        // Alias/case fallback when the context scan missed the note.
        if rest.eq_ignore_ascii_case(dest_route) {
            return true;
        }
    }
    false
}

/// The current managed warning line under `task_line`, when present.
pub(crate) fn existing_warning(
    contents: &str,
    task_line: usize,
) -> Option<String> {
    let lines: Vec<&str> = contents.lines().collect();
    let index = task_line.checked_sub(1)?;
    let task = lines.get(index)?;
    let indent = leading_indent_len(task);
    let end = task_block_end(&lines, index, indent);
    lines[index + 1..end].iter().find_map(|line| {
        let (content, _) = split_line_ending(line);
        content.contains(ROUTE_WARNING_MARKER).then(|| {
            let (trimmed, _) = split_line_ending(content);
            trimmed.to_string()
        })
    })
}

/// Replace, insert, or delete the managed warning child of one task.
///
/// `warning` is the full child line without a line ending. `None` deletes.
/// Returns the new note when the bytes change.
pub(crate) fn apply_warning_edit(
    contents: &str,
    task_line: usize,
    warning: Option<&str>,
) -> Option<String> {
    let physical: Vec<&str> = contents.split_inclusive('\n').collect();
    if physical.is_empty() {
        return None;
    }
    let index = task_line.checked_sub(1)?;
    if index >= physical.len() {
        return None;
    }
    let content_lines: Vec<&str> = contents.lines().collect();
    let task = content_lines.get(index)?;
    let indent = leading_indent_len(task);
    let end = task_block_end(&content_lines, index, indent);
    let existing = content_lines[index + 1..end]
        .iter()
        .position(|line| line.contains(ROUTE_WARNING_MARKER))
        .map(|offset| index + 1 + offset);
    match (warning, existing) {
        (Some(wanted), Some(at)) => {
            let (current, _) = split_line_ending(content_lines[at]);
            if current == wanted {
                return None;
            }
            Some(replace_physical_line(contents, at, wanted))
        }
        (Some(wanted), None) => {
            Some(insert_after_line(contents, index, wanted))
        }
        (None, Some(at)) => Some(remove_physical_line(contents, at)),
        (None, None) => None,
    }
}

fn line_ending_of(physical: &str) -> &'static str {
    if physical.ends_with("\r\n") {
        "\r\n"
    } else if physical.ends_with('\n') {
        "\n"
    } else {
        "\n"
    }
}

fn replace_physical_line(contents: &str, index: usize, line: &str) -> String {
    let mut physical: Vec<String> =
        contents.split_inclusive('\n').map(str::to_string).collect();
    if index >= physical.len() {
        return contents.to_string();
    }
    let ending = line_ending_of(&physical[index]);
    let had_ending = physical[index].ends_with('\n');
    physical[index] = if had_ending {
        format!("{line}{ending}")
    } else {
        line.to_string()
    };
    physical.concat()
}

fn insert_after_line(contents: &str, index: usize, line: &str) -> String {
    let mut physical: Vec<String> =
        contents.split_inclusive('\n').map(str::to_string).collect();
    if index >= physical.len() {
        return contents.to_string();
    }
    let ending = line_ending_of(&physical[index]);
    if !physical[index].ends_with('\n') {
        physical[index].push_str(ending);
    }
    physical.insert(index + 1, format!("{line}{ending}"));
    physical.concat()
}

fn remove_physical_line(contents: &str, index: usize) -> String {
    let mut physical: Vec<String> =
        contents.split_inclusive('\n').map(str::to_string).collect();
    if index >= physical.len() {
        return contents.to_string();
    }
    physical.remove(index);
    physical.concat()
}

/// Whether `line` is an open `#task` row (same statuses scan watches).
fn is_open_task_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    let Some(after) = trimmed.strip_prefix("- [") else {
        return false;
    };
    let status = after.chars().next();
    if !matches!(status, Some(' ') | Some('*') | Some('/') | Some('?')) {
        return false;
    }
    let body = after.get(status.map(|c| c.len_utf8()).unwrap_or(0)..);
    if !body.is_some_and(|b| b.starts_with("] ")) {
        return false;
    }
    line.split_whitespace().any(|token| token == "#task")
}

/// The vault-relative markdown path a wikilink inner text names, with alias,
/// `#heading`/`#^block`, escaped pipe, and leading `/` dropped — the way
/// `scan.rs` matches watching tasks (no `.md` in the source link).
fn link_target_path(inner: &str) -> Option<PathBuf> {
    let before_alias = inner.split('|').next().unwrap_or("");
    let before_alias = before_alias.strip_suffix('\\').unwrap_or(before_alias);
    let target = before_alias.split('#').next().unwrap_or("").trim();
    let target = target.strip_prefix('/').unwrap_or(target);
    if target.is_empty() {
        return None;
    }
    crate::native::vault_links::target_to_markdown_path(target)
}

fn normalized_note_path(note: &str) -> Option<PathBuf> {
    let trimmed = note.trim().trim_start_matches("./").replace('\\', "/");
    let trimmed = trimmed
        .trim()
        .strip_prefix('/')
        .unwrap_or(&trimmed)
        .to_string();
    crate::native::vault_links::target_to_markdown_path(&trimmed)
}

/// Whether wikilink `inner` names mail note `note` (alias/`#` tolerant).
fn link_names_note(inner: &str, note_path: &Path) -> bool {
    let Some(link_path) = link_target_path(inner) else {
        return false;
    };
    let link_slashes = save::path_to_slashes(&link_path);
    let note_slashes = save::path_to_slashes(note_path);
    link_slashes == note_slashes
        || link_slashes.eq_ignore_ascii_case(&note_slashes)
}

/// All 1-based lines of open `#task` rows in `contents` linking `note`,
/// either on the task line or through a direct link-only child bullet.
fn task_lines_for_note(contents: &str, note: &str) -> Vec<usize> {
    let Some(note_path_buf) = normalized_note_path(note) else {
        return Vec::new();
    };
    let note_path = note_path_buf.as_path();
    let lines: Vec<&str> = contents.lines().collect();
    let mut matches = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let (content, _) = split_line_ending(line);
        if !is_open_task_line(content) {
            continue;
        }
        // Links on the task line itself.
        let mut linked =
            crate::native::task_dependencies::raw_wikilink_spans(content)
                .into_iter()
                .filter(|span| !content[..span.open].ends_with('!'))
                .map(|span| &content[span.open + 2..span.end - 2])
                .any(|inner| link_names_note(inner, note_path));
        // Direct link-only child bullets.
        if !linked {
            let task_width = indent_width_columns(content);
            let mut open_items: Vec<usize> = Vec::new();
            let mut child = index + 1;
            while child < lines.len() {
                let (child_content, _) = split_line_ending(lines[child]);
                if child_content.trim().is_empty() {
                    let next = lines[child + 1..]
                        .iter()
                        .position(|l| !l.trim().is_empty())
                        .map(|off| child + 1 + off);
                    if next.is_some_and(|n| {
                        indent_width_columns(lines[n]) > task_width
                    }) {
                        child += 1;
                        continue;
                    }
                    break;
                }
                let width = indent_width_columns(child_content);
                if width <= task_width {
                    break;
                }
                if let Some(inner) = link_only_inner(child_content) {
                    while open_items.last().is_some_and(|o| *o >= width) {
                        open_items.pop();
                    }
                    if open_items.is_empty()
                        && link_names_note(inner, note_path)
                    {
                        linked = true;
                        break;
                    }
                    open_items.push(width);
                } else if is_list_item(child_content) {
                    while open_items.last().is_some_and(|o| *o >= width) {
                        open_items.pop();
                    }
                    // A prose child still opens a nesting level.
                    open_items.push(width);
                }
                child += 1;
            }
        }
        if linked {
            matches.push(index + 1);
        }
    }
    matches
}

fn indent_width_columns(line: &str) -> usize {
    let mut width = 0;
    for c in line.chars() {
        match c {
            ' ' => width += 1,
            '\t' => width += 4 - width % 4,
            _ => break,
        }
    }
    width
}

fn link_only_inner(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix(['-', '*', '+'])?;
    let content = rest.strip_prefix([' ', '\t'])?;
    // Only the bullet content, trimmed; must be exactly one wikilink.
    let content = content.trim();
    let inner = content.strip_prefix("[[")?.strip_suffix("]]")?;
    (!inner.contains("[[") && !inner.contains("]]")).then_some(inner)
}

fn is_list_item(line: &str) -> bool {
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix(['-', '*', '+']) {
        return rest.starts_with([' ', '\t']);
    }
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    digits > 0
        && trimmed[digits..].starts_with(['.', ')'])
        && trimmed[digits + 1..].starts_with([' ', '\t'])
}

/// Locate the open `#task` row for `note` in the current staged contents.
///
/// Prefers `recorded_line` when it still links the note; otherwise takes the
/// unique matching row. Returns `None` when missing or ambiguous (caller
/// skips the intent with a body-free warning).
pub(crate) fn locate_task_line(
    contents: &str,
    note: &str,
    recorded_line: usize,
) -> Option<usize> {
    let candidates = task_lines_for_note(contents, note);
    if candidates.is_empty() {
        return None;
    }
    if candidates.contains(&recorded_line) {
        // Confirm the recorded row is still an open task linking the note.
        return Some(recorded_line);
    }
    if candidates.len() == 1 {
        return Some(candidates[0]);
    }
    None
}

/// 1-based line of the open `#task` in `contents` that links `note`.
/// Matches link targets the way `scan.rs` does (no `.md`, alias/`#`
/// tolerant), not by substring.
#[allow(dead_code)]
pub(crate) fn find_task_line_for_note(
    contents: &str,
    note: &str,
) -> Option<usize> {
    // Unique-match lookup; callers needing recorded-line preference and
    // ambiguity detection use `locate_task_line`.
    let mut found = task_lines_for_note(contents, note).into_iter();
    let first = found.next()?;
    if found.next().is_some() {
        return None;
    }
    Some(first)
}

/// Count open inbox tasks that still carry a managed route warning.
pub(crate) fn open_route_problem_count(
    watchers: &WatchScan,
    inbox: &str,
    vault: &Path,
) -> usize {
    let contents = fs::read_to_string(vault.join(inbox)).unwrap_or_default();
    watchers
        .tasks()
        .iter()
        .filter(|task| is_inbox_path(&task.path, inbox))
        .filter(|task| existing_warning(&contents, task.line).is_some())
        .count()
}

/// Watching / awaiting counts for the doctor row: distinct enrolled
/// threads, and those whose open tasks still carry exact `#awaiting_reply`.
pub(crate) fn watch_counts(
    watchers: &WatchScan,
    enrolled: impl Fn(&ThreadKey) -> bool,
) -> (usize, usize) {
    let watching = watchers.threads().filter(|key| enrolled(key)).count();
    let awaiting = watchers
        .threads()
        .filter(|key| {
            enrolled(key)
                && watchers.watchers(key).any(|task| {
                    task.text
                        .split_whitespace()
                        .any(|token| token == "#awaiting_reply")
                })
        })
        .count();
    (watching, awaiting)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::{
        env::TestEnvGuard,
        gmail::{
            route::RouteProblemKind,
            sync::scan::{LinkPlacement, WatchTarget},
        },
    };

    fn problem() -> RouteProblem {
        RouteProblem {
            kind: RouteProblemKind::UnknownNote,
            label: "#/missing".to_string(),
            message: "unknown note".to_string(),
            hint: "create the note or change the label".to_string(),
        }
    }

    fn isolated_state(prefix: &str) -> (tempfile::TempDir, TestEnvGuard) {
        let temp = tempfile::tempdir().unwrap();
        let _ = prefix;
        let guard = TestEnvGuard::set(&[(
            "XDG_STATE_HOME",
            Some(temp.path().as_os_str()),
        )]);
        (temp, guard)
    }

    #[test]
    fn warning_child_matches_the_filing_format() {
        let line = route_warning_child(&problem());
        assert_eq!(
            line,
            "\t- ⚠️ **GMAIL ROUTE:** `#/missing` unknown note — create the note or change the label"
        );
    }

    #[test]
    fn warning_edit_inserts_replaces_and_deletes() {
        let original =
            "## Tasks\n- [ ] #task [[mail/a|✉️ A]]\n- [ ] #task other\n";
        let wanted = route_warning_child(&problem());
        let inserted = apply_warning_edit(original, 2, Some(&wanted)).unwrap();
        assert!(inserted.contains(&wanted), "{inserted}");
        assert!(inserted.contains("- [ ] #task other"), "{inserted}");
        let replaced = apply_warning_edit(
            &inserted,
            2,
            Some("\t- ⚠️ **GMAIL ROUTE:** `#/job` ok"),
        )
        .unwrap();
        assert!(replaced.contains("#/job"), "{replaced}");
        assert!(!replaced.contains("#/missing"), "{replaced}");
        let cleared = apply_warning_edit(&replaced, 2, None).unwrap();
        assert!(!cleared.contains("GMAIL ROUTE"), "{cleared}");
        assert_eq!(
            apply_warning_edit(original, 2, None),
            None,
            "no warning to delete"
        );
    }

    #[test]
    fn find_task_line_matches_the_mail_note() {
        // Real rows link without `.md`; alias and `#heading` tolerant.
        let contents = "## Tasks\n- [ ] #task [[mail/2026/a|✉️ A]]\n";
        assert_eq!(
            find_task_line_for_note(contents, "mail/2026/a.md"),
            Some(2)
        );
        let aliased = "## Tasks\n- [ ] #task [[mail/2026/a|✉️ A]]\n";
        assert_eq!(find_task_line_for_note(aliased, "mail/2026/a.md"), Some(2));
        let headed = "## Tasks\n- [ ] #task [[mail/2026/a#Notes|✉️ A]]\n";
        assert_eq!(find_task_line_for_note(headed, "mail/2026/a.md"), Some(2));
        assert_eq!(find_task_line_for_note(contents, "mail/2026/b.md"), None);
        // Ambiguous rows do not resolve.
        let two = "## Tasks\n- [ ] #task [[mail/a|✉️ A]]\n- [ ] #task [[mail/a|✉️ A]]\n";
        assert_eq!(find_task_line_for_note(two, "mail/a.md"), None);
    }

    #[test]
    fn locate_prefers_recorded_line_then_unique_row() {
        let contents = "## Tasks\n- [ ] #task [[mail/a|✉️ A]]\n- [ ] #task [[mail/b|✉️ B]]\n";
        // Recorded line still matches.
        assert_eq!(locate_task_line(contents, "mail/b.md", 3), Some(3));
        // Stale recorded line falls through to the unique match.
        assert_eq!(locate_task_line(contents, "mail/b.md", 2), Some(3));
        // Child-bullet links count.
        let child = "## Tasks\n- [ ] #task Root\n\t- [[mail/a]]\n- [ ] #task [[mail/b]]\n";
        assert_eq!(locate_task_line(child, "mail/a.md", 2), Some(2));
        // Ambiguous or missing skips.
        let two = "## Tasks\n- [ ] #task [[mail/a]]\n- [ ] #task [[mail/a]]\n";
        assert_eq!(locate_task_line(two, "mail/a.md", 99), None);
        assert_eq!(locate_task_line(contents, "mail/missing.md", 2), None);
        // Closed rows never match.
        let closed = "## Tasks\n- [x] #task [[mail/a]]\n";
        assert_eq!(locate_task_line(closed, "mail/a.md", 2), None);
    }

    #[test]
    fn busy_skip_streak_vectors() {
        let (_temp, _env) = isolated_state("gmail-health-streak");
        // No state: first skip starts the streak, no alarm.
        assert!(note_busy_skip("the Gmail sync guard")
            .expect("skip")
            .is_none());
        let state = load_state().unwrap();
        assert!(state.skipping_since.is_some(), "streak starts");
        assert!(state.last_success_at.is_none());
        // Still within the window: quiet.
        assert!(note_busy_skip("the Gmail sync guard")
            .expect("skip")
            .is_none());
        // A skip 2h1m after the streak start alarms with "never".
        let old_skip = (save::current_utc()
            - chrono::Duration::hours(2)
            - chrono::Duration::minutes(1))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let mut state = load_state().unwrap();
        state.skipping_since = Some(old_skip);
        write_state(&state).unwrap();
        let alarm = note_busy_skip("the Gmail sync guard")
            .expect("skip")
            .expect("alarms");
        assert!(alarm.contains("never"), "{alarm}");
        assert!(alarm.contains("bob gmail doctor"), "{alarm}");

        // Success clears the streak.
        record_success().expect("success");
        let cleared = load_state().unwrap();
        assert!(cleared.skipping_since.is_none());
        assert!(cleared.last_success_at.is_some());
        // With a success at T, the first skip at T+10h does not alarm.
        let mut state = load_state().unwrap();
        let success_t = (save::current_utc() - chrono::Duration::hours(10))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        state.last_success_at = Some(success_t);
        state.skipping_since = None;
        write_state(&state).unwrap();
        assert!(note_busy_skip("the Gmail sync guard")
            .expect("skip")
            .is_none());
        let restarted = load_state().unwrap();
        assert!(restarted.skipping_since.is_some());
        // A skip at T+12h1m (streak 2h1m, still after the success) alarms.
        let mut state = load_state().unwrap();
        // Keep the T success; push the streak start to 2h1m ago.
        let old = (save::current_utc()
            - chrono::Duration::hours(2)
            - chrono::Duration::minutes(1))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        state.skipping_since = Some(old);
        write_state(&state).unwrap();
        let alarm = note_busy_skip("the Gmail sync guard")
            .expect("skip")
            .expect("alarms");
        assert!(alarm.contains("bob gmail doctor"), "{alarm}");
        assert!(!alarm.contains("never"), "{alarm}");
    }

    #[test]
    fn winning_label_reports_the_actual_prefixed_case() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        fs::write(
            vault.join("job.md"),
            "---\ntype: \"[[area]]\"\n---\n## Tasks\n",
        )
        .unwrap();
        let ctx = RouteContext::load(vault);
        let mut labels = LabelsById::new();
        labels.insert(
            "Label_1".into(),
            crate::native::gmail::gog::GmailLabel {
                id: "Label_1".into(),
                name: "+/Job".into(),
                kind: Some("user".into()),
            },
        );
        let config = HealthConfig {
            labels_by_id: labels,
            enrollment_label: Some("bob".into()),
            route_prefixes: vec!["+".into(), "#".into()],
        };
        let ids = BTreeSet::from(["Label_1".to_string()]);
        assert_eq!(winning_label(&config, &ids, "job", &ctx), "+/Job");
    }

    #[test]
    fn inbox_paths_normalize_slashes_and_dot_prefix() {
        assert!(is_inbox_path("gmail_inbox.md", "gmail_inbox.md"));
        assert!(is_inbox_path("./gmail_inbox.md", "gmail_inbox.md"));
        assert!(!is_inbox_path("job.md", "gmail_inbox.md"));
    }

    #[test]
    fn sync_state_round_trip_and_staleness() {
        let (_temp, _env) = isolated_state("gmail-health-state");
        assert!(stale_skip_message("the Gmail sync guard").is_none());
        record_success().expect("write success");
        assert!(stale_skip_message("the Gmail sync guard").is_none());
        let mut state = load_state().unwrap();
        let old = (save::current_utc() - chrono::Duration::hours(3))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        state.last_success_at = Some(old.clone());
        write_state(&state).unwrap();
        let message =
            stale_skip_message("the Gmail sync guard").expect("stale");
        assert!(message.contains("3 h"), "{message}");
        assert!(message.contains(&old), "{message}");
        assert!(message.contains("bob gmail doctor"), "{message}");
    }

    #[test]
    fn plan_refiles_inbox_rows_and_reports_outside_conflicts_once() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        fs::write(
            vault.join("gmail_inbox.md"),
            "## Tasks\n- [ ] #task [[mail/a|✉️ A]]\n\t- ⚠️ **GMAIL ROUTE:** `#/job` unknown note\n",
        )
        .unwrap();
        fs::write(
            vault.join("job.md"),
            "---\ntype: \"[[area]]\"\n---\n## Tasks\n",
        )
        .unwrap();
        fs::write(
            vault.join("body.md"),
            "---\ntype: \"[[area]]\"\n---\n## Tasks\n",
        )
        .unwrap();
        let key = ThreadKey::new("you@example.com", "aabbcc").unwrap();
        let inbox_task = WatchingTask {
            path: "gmail_inbox.md".into(),
            line: 2,
            text: "- [ ] #task [[mail/a|✉️ A]]".into(),
            block_id: None,
            status: ' ',
            targets: vec![WatchTarget {
                key: key.clone(),
                note: "mail/a.md".into(),
                placement: LinkPlacement::TaskLine,
            }],
        };
        let outside = WatchingTask {
            path: "body.md".into(),
            line: 4,
            text: "- [ ] #task [[mail/a|✉️ A]]".into(),
            block_id: None,
            status: ' ',
            targets: inbox_task.targets.clone(),
        };
        let watchers =
            WatchScan::from_tasks_for_test(vec![inbox_task, outside]);
        // Enrollment fold is easier through a real journal file.
        let dir = vault.join(".bob/gmail");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("host.jsonl"),
            serde_json::to_string(&json!({
                "v": 1,
                "ts": "2026-11-07T12:00:00-05:00",
                "host": "host",
                "kind": "gmail.filed",
                "value": {
                    "account": "you@example.com",
                    "thread_id": "aabbcc",
                    "note": "mail/a.md",
                    "task_file": "gmail_inbox.md",
                    "via": "problem"
                }
            }))
            .unwrap()
                + "\n",
        )
        .unwrap();
        let enrollment =
            crate::native::gmail::sync::enroll::EnrollmentFold::load(vault)
                .unwrap();
        let mut labels = LabelsById::new();
        labels.insert(
            "Label_1".into(),
            crate::native::gmail::gog::GmailLabel {
                id: "Label_1".into(),
                name: "#/job".into(),
                kind: Some("user".into()),
            },
        );
        let config = HealthConfig {
            labels_by_id: labels,
            enrollment_label: Some("bob".into()),
            route_prefixes: vec!["#".into()],
        };
        let ctx = RouteContext::load(vault);
        let mut ids = BTreeMap::new();
        ids.insert(key.clone(), BTreeSet::from(["Label_1".into()]));
        let intents = plan(
            vault,
            "gmail_inbox.md",
            &enrollment,
            &watchers,
            &ids,
            &config,
            &ctx,
            &BTreeSet::new(),
        );
        assert!(
            intents.iter().any(|intent| {
                matches!(
                    &intent.action,
                    HealthAction::Refile { route, .. } if route == "job"
                )
            }),
            "{intents:?}"
        );
        assert!(
            intents.iter().any(|intent| {
                matches!(intent.action, HealthAction::Conflict { .. })
                    && intent.task_path == "body.md"
            }),
            "{intents:?}"
        );
    }
}

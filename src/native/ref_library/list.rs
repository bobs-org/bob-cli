//! Filtered library views for `bob ref list`.
//!
//! The default view (no filter option at all) is the reading queue
//! (`queued` and `started`); any filter option searches every reading
//! state unless `-R/--reading-state` narrows it. Values within one option
//! are ORed, different options are ANDed. Superseded notes are always
//! excluded and counted in `hidden.superseded`. `--limit 50` caps every
//! format and `-A/--all` lifts it. `-g/--git-dates` backfills missing
//! modern dates from one vault `git log` pass over `^ref` tracker lines.

use std::process::Command;

use super::{LibraryConfig, RefRow};
use crate::native::env as bob_env;
use crate::native::highlights_ref::ref_task_mark_status;

/// Reading states in `bob ref list` display order: started, queued,
/// finished, dropped, unknown.
pub(crate) const LIST_STATE_ORDER: [&str; 5] =
    ["started", "queued", "finished", "dropped", "unknown"];

/// The default view: the reading queue.
pub(crate) const DEFAULT_QUEUE_STATES: [&str; 2] = ["queued", "started"];

/// The effective `bob ref list` selection: normalized filters plus the
/// resolved `--since` cutoff.
#[derive(Debug, Clone)]
pub(crate) struct ListSelection {
    /// Effective reading states in [`LIST_STATE_ORDER`]. All five means
    /// no state filtering.
    pub states: Vec<String>,
    /// True when no filter option was given and the queue default applied.
    pub defaulted: bool,
    pub statuses: Option<Vec<String>>,
    pub ref_types: Option<Vec<String>>,
    pub origin: Option<String>,
    pub parent: Option<String>,
    /// The `--since` value as given.
    pub since: Option<String>,
    /// The resolved `YYYY-MM-DD` cutoff, or `None` without `--since`.
    pub since_cutoff: Option<String>,
    /// The row cap, or `None` with `-A/--all`.
    pub limit: Option<u64>,
}

impl ListSelection {
    /// True when `-s/--status` was given (drives legacy collapse).
    pub(crate) fn has_status_filter(&self) -> bool {
        self.statuses.is_some()
    }
}

/// Build the effective selection. `states` is the raw `-R` values (or
/// `None`); `all` inside `-R` means every state. With no filter option at
/// all the queue default applies in every format.
#[allow(clippy::too_many_arguments)]
pub(crate) fn select(
    states: Option<Vec<String>>,
    statuses: Option<Vec<String>>,
    ref_types: Option<Vec<String>>,
    origin: Option<String>,
    parent: Option<String>,
    since: Option<String>,
    since_cutoff: Option<String>,
    limit: Option<u64>,
) -> ListSelection {
    let has_filter = states.is_some()
        || statuses.is_some()
        || ref_types.is_some()
        || origin.is_some()
        || parent.is_some()
        || since.is_some();
    let (selected, defaulted) = match states {
        Some(values) => {
            let mut kept = Vec::new();
            for state in &LIST_STATE_ORDER {
                if values.iter().any(|value| value == state)
                    || values.iter().any(|value| value == "all")
                {
                    kept.push(state.to_string());
                }
            }
            (kept, false)
        }
        None if !has_filter => (
            DEFAULT_QUEUE_STATES.iter().map(|s| s.to_string()).collect(),
            true,
        ),
        None => (
            LIST_STATE_ORDER.iter().map(|s| s.to_string()).collect(),
            false,
        ),
    };
    ListSelection {
        states: selected,
        defaulted,
        statuses,
        ref_types,
        origin,
        parent,
        since,
        since_cutoff,
        limit,
    }
}

/// The row date for filtering and ordering: `finished` for the finished
/// and dropped states, `added` otherwise.
pub(crate) fn row_date(row: &RefRow) -> Option<&str> {
    match row.reading_state.as_str() {
        "finished" | "dropped" => row.finished.as_deref(),
        _ => row.added.as_deref(),
    }
}

/// Validate a `--since` value's shape: `YYYY-MM-DD` or `<N>d|w|m|y`.
/// Relative values resolve against the clock at render time.
pub(crate) fn validate_since(value: &str) -> Result<String, String> {
    if parse_since_cutoff(value, &bob_env::current_datetime().date()).is_some()
    {
        Ok(value.to_string())
    } else {
        Err(format!(
            "invalid --since {value:?}: expected YYYY-MM-DD or <N>d|w|m|y"
        ))
    }
}

/// Resolve a `--since` value to a `YYYY-MM-DD` cutoff relative to `today`.
/// Calendar months use chrono `checked_sub_months`; years are twelve
/// months, weeks are seven days.
pub(crate) fn parse_since_cutoff(
    value: &str,
    today: &chrono::NaiveDate,
) -> Option<String> {
    if let Ok(date) =
        chrono::NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d")
    {
        return Some(date.format("%Y-%m-%d").to_string());
    }
    let trimmed = value.trim();
    let (digits, unit) = trimmed.split_at(
        trimmed
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(trimmed.len()),
    );
    if digits.is_empty() || unit.len() != 1 {
        return None;
    }
    let amount: u32 = digits.parse().ok()?;
    let cutoff = match unit {
        "d" => today.checked_sub_days(chrono::Days::new(u64::from(amount)))?,
        "w" => today.checked_sub_days(chrono::Days::new(
            u64::from(amount).checked_mul(7)?,
        ))?,
        "m" => today.checked_sub_months(chrono::Months::new(amount))?,
        "y" => today
            .checked_sub_months(chrono::Months::new(amount.checked_mul(12)?))?,
        _ => return None,
    };
    Some(cutoff.format("%Y-%m-%d").to_string())
}

/// The outcome of filtering: ordered row indices plus envelope counts.
#[derive(Debug, Default)]
pub(crate) struct ListOutcome {
    /// Non-superseded rows passing every filter, in list order.
    pub ordered: Vec<usize>,
    /// Superseded rows that pass the attribute filters.
    pub hidden_superseded: usize,
    /// Rows dropped by `--since` for lack of a row date.
    pub undated_excluded: usize,
}

/// Filter and order the index rows for one `bob ref list` run.
pub(crate) fn filter_list(
    rows: &[RefRow],
    selection: &ListSelection,
) -> ListOutcome {
    let mut outcome = ListOutcome::default();
    let mut kept = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        if row.superseded_by.is_some() {
            if matches_attributes(row, selection) {
                outcome.hidden_superseded += 1;
            }
            continue;
        }
        if !matches_attributes(row, selection) {
            continue;
        }
        if let Some(cutoff) = selection.since_cutoff.as_deref() {
            match row_date(row) {
                Some(date) if date >= cutoff => {}
                Some(_) => continue,
                None => {
                    outcome.undated_excluded += 1;
                    continue;
                }
            }
        }
        kept.push(index);
    }
    kept.sort_by(|a, b| compare_rows(&rows[*a], &rows[*b]));
    outcome.ordered = kept;
    outcome
}

/// Attribute filters: reading state, status, type, origin, parent.
/// Values within one option are ORed; options are ANDed.
fn matches_attributes(row: &RefRow, selection: &ListSelection) -> bool {
    if selection.states.len() < LIST_STATE_ORDER.len()
        && !selection
            .states
            .iter()
            .any(|state| state == &row.reading_state)
    {
        return false;
    }
    if let Some(statuses) = selection.statuses.as_deref()
        && !statuses
            .iter()
            .any(|status| status == row.status.as_deref().unwrap_or("unknown"))
    {
        return false;
    }
    if let Some(types) = selection.ref_types.as_deref()
        && !row.ref_type.as_deref().is_some_and(|ref_type| {
            types
                .iter()
                .any(|wanted| wanted.eq_ignore_ascii_case(ref_type))
        })
    {
        return false;
    }
    if let Some(origin) = selection.origin.as_deref()
        && row.origin != origin
    {
        return false;
    }
    if let Some(parent) = selection.parent.as_deref()
        && row.parent.as_deref() != Some(parent)
    {
        return false;
    }
    true
}

/// List order: reading state (started, queued, finished, dropped,
/// unknown), modern before legacy, `next` before `ready` within queued,
/// row date newest first with undated last, then title.
fn compare_rows(left: &RefRow, right: &RefRow) -> std::cmp::Ordering {
    list_state_rank(&left.reading_state)
        .cmp(&list_state_rank(&right.reading_state))
        .then_with(|| (left.era == "legacy").cmp(&(right.era == "legacy")))
        .then_with(|| {
            queue_status_rank(left.status.as_deref())
                .cmp(&queue_status_rank(right.status.as_deref()))
        })
        .then_with(|| match (row_date(left), row_date(right)) {
            (Some(a), Some(b)) => b.cmp(a),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        })
        .then_with(|| {
            left.title
                .to_lowercase()
                .cmp(&right.title.to_lowercase())
                .then_with(|| left.path.cmp(&right.path))
        })
}

fn list_state_rank(state: &str) -> u8 {
    LIST_STATE_ORDER
        .iter()
        .position(|s| *s == state)
        .unwrap_or(LIST_STATE_ORDER.len()) as u8
}

fn queue_status_rank(status: Option<&str>) -> u8 {
    match status {
        Some("next") => 0,
        Some("ready") => 1,
        _ => 2,
    }
}

/// Fill missing `added`/`finished` dates from one vault `git log` pass
/// over `^ref` tracker lines. Only modern notes are filled, only missing
/// values, with source `git`. Returns the coverage word and an optional
/// one-line stderr warning; Git trouble is never fatal.
pub(crate) fn fill_git_dates(
    config: &LibraryConfig,
    rows: &mut [RefRow],
) -> (Option<String>, Option<String>) {
    let wanted = rows.iter().any(|row| {
        row.era == "modern"
            && (row.added.is_none()
                || matches!(row.reading_state.as_str(), "finished" | "dropped")
                    && row.finished.is_none())
    });
    if !wanted {
        return (None, None);
    }
    let history = match load_ref_history(config) {
        Ok(history) => history,
        Err(reason) => {
            return (
                Some("unavailable".to_string()),
                Some(format!(
                    "git history unavailable ({reason}); dates left as stored"
                )),
            );
        }
    };
    for row in rows.iter_mut().filter(|row| row.era == "modern") {
        // `load_ref_history` returns newest-first; the earliest add is
        // the last record for the path.
        if row.added.is_none()
            && let Some(date) = history
                .iter()
                .rfind(|(path, _, _)| path == &row.path)
                .map(|(_, date, _)| date.clone())
        {
            row.added = Some(date);
            row.added_source = Some("git".to_string());
        }
        if matches!(row.reading_state.as_str(), "finished" | "dropped")
            && row.finished.is_none()
            && let Some(date) =
                history.iter().find_map(|(path, date, marks)| {
                    (path == &row.path
                        && marks.iter().any(|mark| {
                            ref_task_mark_status(*mark) == row.status.as_deref()
                        }))
                    .then_some(date.clone())
                })
        {
            row.finished = Some(date);
            row.finished_source = Some("git".to_string());
        }
    }
    (None, None)
}

/// One `(vault-relative path, commit date, added `^ref` marks)` triple per
/// commit touching a path, newest commit first.
type RefHistory = Vec<(String, String, Vec<char>)>;

/// Run one `git log` pass over the ref dir and parse it per file: the
/// earliest commit that added a `^ref` line gives `added`, the newest
/// commit whose added `^ref` line carries the current finished or dropped
/// mark gives `finished`. Diff paths are repo-root-relative, so the vault
/// root is resolved against `git rev-parse --show-toplevel` first; a
/// vault nested inside a larger repo still matches its own rows.
fn load_ref_history(config: &LibraryConfig) -> Result<RefHistory, String> {
    let root = git_root(&config.bob_dir)?;
    let forward =
        |path: &std::path::Path| path.to_string_lossy().replace('\\', "/");
    let spec = config
        .ref_dir
        .strip_prefix(&config.bob_dir)
        .map(forward)
        .unwrap_or_else(|_| forward(&config.ref_dir));
    let output = Command::new("git")
        .arg("-C")
        .arg(&config.bob_dir)
        .arg("log")
        .arg("--format=%x1e%cs")
        .arg("-p")
        .arg("-U0")
        .arg("--no-color")
        .arg("--no-ext-diff")
        .arg("-G\\^ref")
        .arg("--")
        .arg(&spec)
        .output()
        .map_err(|error| format!("could not run git: {error}"))?;
    if !output.status.success() {
        return Err("git log over the ref dir failed".to_string());
    }
    let prefix = config
        .bob_dir
        .strip_prefix(&root)
        .map(forward)
        .unwrap_or_default();
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(parse_ref_history(&text)
        .into_iter()
        .filter_map(|(path, date, marks)| {
            let unprefixed =
                prefix.is_empty().then(|| path.clone()).or_else(|| {
                    path.strip_prefix(&format!("{prefix}/")).map(str::to_string)
                })?;
            Some((unprefixed, date, marks))
        })
        .collect())
}

/// The vault's Git repo root, or an unavailable reason.
fn git_root(bob_dir: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(bob_dir)
        .arg("rev-parse")
        .arg("--show-toplevel")
        .output()
        .map_err(|error| format!("could not run git: {error}"))?;
    if !output.status.success() {
        return Err("vault is not inside a git repository".to_string());
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if root.is_empty() {
        return Err("git reported no repository root".to_string());
    }
    Ok(std::path::PathBuf::from(root))
}

/// Parse `git log` output into per-commit `(path, date, marks)` triples.
/// Each `%x1e%cs` record opens with its commit date; `+++ b/<path>`
/// headers name the file; added lines (starting with `+`, not `+++`)
/// carrying a `^ref` token contribute their checkbox mark.
fn parse_ref_history(text: &str) -> RefHistory {
    let mut history = Vec::new();
    for record in text.split('\x1e') {
        let mut lines = record.lines();
        let Some(date) = lines.next().map(str::trim).filter(|d| {
            chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok()
        }) else {
            continue;
        };
        let mut path: Option<String> = None;
        let mut marks: Vec<char> = Vec::new();
        let flush = |history: &mut RefHistory,
                     path: &mut Option<String>,
                     marks: &mut Vec<char>| {
            if let Some(path) = path.take() {
                history.push((path, date.to_string(), std::mem::take(marks)));
            }
        };
        for line in lines {
            if let Some(rest) = line.strip_prefix("+++ b/") {
                flush(&mut history, &mut path, &mut marks);
                path = Some(rest.trim().to_string());
            } else if line.starts_with('+')
                && !line.starts_with("+++")
                && line.split_whitespace().any(|token| token == "^ref")
            {
                if let Some(mark) = added_mark(line) {
                    marks.push(mark);
                }
            } else if line.starts_with("diff --git") {
                flush(&mut history, &mut path, &mut marks);
            }
        }
        flush(&mut history, &mut path, &mut marks);
    }
    history
}

/// The checkbox mark of an added tracker line (`- [x] …`), if any.
fn added_mark(line: &str) -> Option<char> {
    let body = line.strip_prefix('+').unwrap_or(line);
    let start = body.find('[')?;
    let mut chars = body[start + 1..].chars();
    let mark = chars.next()?;
    if chars.next() == Some(']') {
        Some(mark)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(value: &str) -> chrono::NaiveDate {
        chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("test date")
    }

    #[test]
    fn since_accepts_absolute_and_relative_dates() {
        let today = day("2026-10-06");
        assert_eq!(
            parse_since_cutoff("2026-09-01", &today).as_deref(),
            Some("2026-09-01")
        );
        assert_eq!(
            parse_since_cutoff("30d", &today).as_deref(),
            Some("2026-09-06")
        );
        assert_eq!(
            parse_since_cutoff("4w", &today).as_deref(),
            Some("2026-09-08")
        );
        // Calendar months, not thirty days.
        assert_eq!(
            parse_since_cutoff("6m", &today).as_deref(),
            Some("2026-04-06")
        );
        assert_eq!(
            parse_since_cutoff("1y", &today).as_deref(),
            Some("2025-10-06")
        );
        assert!(parse_since_cutoff("yesterday", &today).is_none());
        assert!(parse_since_cutoff("2026-13-01", &today).is_none());
        assert!(parse_since_cutoff("10x", &today).is_none());
        assert!(parse_since_cutoff("", &today).is_none());
    }

    #[test]
    fn selection_defaults_to_the_queue_without_filters() {
        let defaulted =
            select(None, None, None, None, None, None, None, Some(50));
        assert!(defaulted.defaulted);
        assert_eq!(defaulted.states, vec!["queued", "started"]);
        assert_eq!(defaulted.limit, Some(50));

        // Any filter option searches every state unless -R narrows it.
        let unfiltered = select(
            None,
            None,
            Some(vec!["chat".to_string()]),
            None,
            None,
            None,
            None,
            None,
        );
        assert!(!unfiltered.defaulted);
        assert_eq!(unfiltered.states.len(), LIST_STATE_ORDER.len());
        assert_eq!(unfiltered.limit, None);

        // `all` inside -R means every state.
        let all = select(
            Some(vec!["all".to_string()]),
            None,
            None,
            None,
            None,
            None,
            None,
            Some(50),
        );
        assert!(!all.defaulted);
        assert_eq!(all.states.len(), LIST_STATE_ORDER.len());
    }

    #[test]
    fn ref_history_parses_per_file_marks_newest_first() {
        let text = "\x1e2026-10-02\n+++ b/ref/papers/new.md\n+- [x] #task #ref [[lib/papers/new.pdf]] #hide ^ref\n\x1e2026-09-10\n+++ b/ref/papers/new.md\n+- [ ] #task #ref [[lib/papers/new.pdf]] #hide ^ref\n";
        let history = parse_ref_history(text);
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].0, "ref/papers/new.md");
        assert_eq!(history[0].1, "2026-10-02");
        assert_eq!(history[0].2, vec!['x']);
        assert_eq!(history[1].1, "2026-09-10");
        assert_eq!(history[1].2, vec![' ']);
    }
}

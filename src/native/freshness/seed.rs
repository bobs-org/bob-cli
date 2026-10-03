//! The guarded, idempotent, staggered cutover seed behind
//! `bob freshness seed` (`docs/freshness.md` §7).
//!
//! Ready tasks without a valid `fresh` are bin-packed by note into 7
//! buckets, oldest first, so nothing is due on cutover day; every
//! other open, non-recurring task gets today so returning deferrals
//! arrive as RESURFACED or ROTTEN, never NEW. The seed aborts with no
//! writes when a second seed is detected (unless `--force`), when any
//! changed line would parse differently, or when a file changed since
//! the scan.

use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::Path,
};

use chrono::{Days, NaiveDate};
use serde::Serialize;

use super::super::{
    config::FreshnessConfig, dataview::tasks_fingerprint,
    task_status_hooks::task_metadata,
};
use super::{
    placement::{read_freshness, stamp_fresh_preserve_keeps},
    scan::{RowCtx, Snapshot},
    state::evaluate,
};

/// How a seed candidate is stamped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeedKind {
    /// An in-scope Ready task: staggered bucket date.
    Ready,
    /// Any other open, non-recurring task: today.
    Other,
}

/// One line the seed will rewrite.
#[derive(Debug, Clone)]
struct SeedChange {
    path: String,
    line: u32,
    old_line: String,
    new_line: String,
    kind: SeedKind,
}

/// One stagger bucket: bucket `index` (1-based) lands on
/// `today − 7 + index`.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct SeedBucket {
    pub(crate) fresh: String,
    pub(crate) due_on: String,
    pub(crate) count: u32,
    pub(crate) notes: BTreeMap<String, u32>,
}

/// The seed outcome for human and JSON output.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct SeedReport {
    pub(crate) date: String,
    pub(crate) dry_run: bool,
    pub(crate) stamped_ready: u32,
    pub(crate) stamped_other: u32,
    pub(crate) buckets: Vec<SeedBucket>,
    pub(crate) skipped_already_stamped: u32,
    pub(crate) skipped_recurring: u32,
    pub(crate) skipped_out_of_scope: u32,
    pub(crate) files: Vec<String>,
}

/// Why the seed refused to run.
#[derive(Debug, Clone)]
pub(crate) enum SeedError {
    /// Stamps dated before today exist: seeding after cutover would
    /// mark every capture since then as reviewed.
    AlreadySeeded { count: u32 },
    /// A changed line would parse differently under either Rust
    /// parser; no file was written.
    InvarianceAborted { lines: Vec<String> },
    /// A file changed since the scan; no file was written.
    ConcurrentChange { path: String },
    /// A line the seed must stamp refuses placement; no file was
    /// written.
    Refused {
        path: String,
        line: u32,
        reason: String,
    },
    /// A write failed after earlier files were written; the vault's
    /// git history is the rollback.
    WriteFailed {
        path: String,
        message: String,
        written: Vec<String>,
    },
}

impl SeedError {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::AlreadySeeded { count } => format!(
                "bob freshness seed: refusing: {count} task(s) already \
                carry a fresh date before today; seeding after cutover \
                would mark every capture since then as reviewed \
                (pass -F/--force to override)"
            ),
            Self::InvarianceAborted { lines } => format!(
                "bob freshness seed: refusing: {} line(s) would parse \
                differently after stamping; no files written:\n  {}",
                lines.len(),
                lines.join("\n  ")
            ),
            Self::ConcurrentChange { path } => format!(
                "bob freshness seed: refusing: {path} changed since the \
                scan; no files written"
            ),
            Self::Refused { path, line, reason } => format!(
                "bob freshness seed: refusing: {path}:{line} cannot be \
                stamped ({reason}); no files written"
            ),
            Self::WriteFailed {
                path,
                message,
                written,
            } => format!(
                "bob freshness seed: failed writing {path}: {message} \
                (already written: {})",
                if written.is_empty() {
                    "none".to_string()
                } else {
                    written.join(", ")
                }
            ),
        }
    }
}

/// Plan and, unless `dry_run`, apply the cutover seed.
pub(crate) fn run_seed(
    snapshot: &Snapshot,
    bob_dir: &Path,
    dry_run: bool,
    force: bool,
) -> Result<SeedReport, SeedError> {
    let today = snapshot.today;
    let config = &snapshot.config;

    let ready_set: std::collections::BTreeSet<(String, u32)> = snapshot
        .ready
        .iter()
        .map(|row| (row.task.path.clone(), row.task.line))
        .collect();

    let mut ready_candidates: Vec<&RowCtx> = Vec::new();
    let mut other_candidates: Vec<&RowCtx> = Vec::new();
    let mut skipped_already_stamped = 0;
    let mut skipped_recurring = 0;
    let mut skipped_out_of_scope = 0;
    let mut old_stamp_count = 0;

    for row in &snapshot.open {
        // Path-level exclusions first, then the per-task ones: a
        // whole-token #hide and canonical daily notes never seed.
        // Recurring tasks never stamp (the tickler resurfaces them).
        if is_seed_excluded(&row.task.path) || is_task_excluded(row) {
            skipped_out_of_scope += 1;
            continue;
        }
        if row.task.is_recurring {
            skipped_recurring += 1;
            continue;
        }
        let read = read_freshness(&row.task.original_markdown, today);
        match read.fresh {
            Some(date) if date < today => {
                old_stamp_count += 1;
                skipped_already_stamped += 1;
            }
            Some(_) => {
                skipped_already_stamped += 1;
            }
            None => {
                // Ready candidates are in-scope Ready tasks: engine
                // lane-visible and neither a daily note nor Today.
                // Every other open task — Next, Pending, Blocked, and
                // out-of-scope Ready — lands in the today bucket.
                if ready_set.contains(&(row.task.path.clone(), row.task.line))
                    && !row.is_today
                {
                    ready_candidates.push(row);
                } else {
                    other_candidates.push(row);
                }
            }
        }
    }

    // A same-day rerun finds no candidates: an idempotent no-op that
    // reports zeros rather than refusing on its own bucket dates.
    if (!ready_candidates.is_empty() || !other_candidates.is_empty())
        && old_stamp_count > 0
        && !force
    {
        return Err(SeedError::AlreadySeeded {
            count: old_stamp_count,
        });
    }

    let buckets = bucket_ready(&ready_candidates, today);
    let mut changes =
        Vec::with_capacity(ready_candidates.len() + other_candidates.len());

    for (date, rows) in buckets.dates.iter().zip(&buckets.members) {
        for row in rows {
            let stamp = seed_stamp(row, date, today, config);
            changes.push(stamp_change(row, stamp, SeedKind::Ready)?);
        }
    }
    for row in other_candidates {
        changes.push(stamp_change(row, today, SeedKind::Other)?);
    }

    check_invariance(&changes)?;

    let mut files: Vec<String> = changes
        .iter()
        .map(|change| change.path.clone())
        .collect::<std::collections::BTreeSet<String>>()
        .into_iter()
        .collect();
    files.sort();

    if !dry_run {
        write_changes(bob_dir, snapshot, &changes)?;
    }

    let bucket_reports = buckets.reports(config.interval);
    Ok(SeedReport {
        date: today.format("%Y-%m-%d").to_string(),
        dry_run,
        stamped_ready: changes
            .iter()
            .filter(|change| change.kind == SeedKind::Ready)
            .count() as u32,
        stamped_other: changes
            .iter()
            .filter(|change| change.kind == SeedKind::Other)
            .count() as u32,
        buckets: bucket_reports,
        skipped_already_stamped,
        skipped_recurring,
        skipped_out_of_scope,
        files,
    })
}

/// Notes the seed never touches: `#hide` is per-task (checked from
/// the row's tags), these are path-level.
fn is_seed_excluded(path: &str) -> bool {
    path.split('/').any(|segment| {
        segment == "_templates"
            || segment == "_conflicts"
            || segment.starts_with('.')
    })
}

/// A task the seed skips for a per-task reason: a whole-token `#hide`
/// or a canonical daily note.
fn is_task_excluded(row: &RowCtx) -> bool {
    row.task.tags.iter().any(|tag| tag == "#hide") || row.is_daily_note
}

fn stamp_change(
    row: &RowCtx,
    date: NaiveDate,
    kind: SeedKind,
) -> Result<SeedChange, SeedError> {
    // Preserve mode: the seed stamps dates but must never reset an
    // existing keep streak — the generic stamp default clears keeps.
    let stamp = stamp_fresh_preserve_keeps(&row.task.original_markdown, date);
    if let Some(refused) = stamp.refused {
        return Err(SeedError::Refused {
            path: row.task.path.clone(),
            line: row.task.line,
            reason: refused.as_str().to_string(),
        });
    }
    Ok(SeedChange {
        path: row.task.path.clone(),
        line: row.task.line,
        old_line: row.task.original_markdown.clone(),
        new_line: stamp.line,
        kind,
    })
}

/// The stamp for one Ready candidate: the bucket date, raised so the
/// task is neither due on cutover day nor RESURFACED right away, and
/// clamped to today.
fn seed_stamp(
    row: &RowCtx,
    bucket_date: &NaiveDate,
    today: NaiveDate,
    config: &FreshnessConfig,
) -> NaiveDate {
    let evaluated = evaluate(&row.freshness_row(true), today, config);
    let mut stamp = *bucket_date;
    // fresh + interval > today keeps the task FRESH past cutover.
    if let Some(minimum) = today
        .checked_sub_days(Days::new(u64::from(evaluated.interval_days) - 1))
        && minimum > stamp
    {
        stamp = minimum;
    }
    // fresh >= scheduled avoids an instant RESURFACED tickler.
    if let Some(scheduled) = row.task.scheduled
        && scheduled <= today
        && scheduled > stamp
    {
        stamp = scheduled;
    }
    stamp.min(today)
}

struct ReadyBuckets<'a> {
    dates: Vec<NaiveDate>,
    members: Vec<Vec<&'a RowCtx>>,
}

impl<'a> ReadyBuckets<'a> {
    fn reports(&self, interval: u16) -> Vec<SeedBucket> {
        self.dates
            .iter()
            .zip(&self.members)
            .map(|(date, rows)| {
                let mut notes: BTreeMap<String, u32> = BTreeMap::new();
                for row in rows {
                    *notes.entry(row.task.path.clone()).or_default() += 1;
                }
                SeedBucket {
                    fresh: date.format("%Y-%m-%d").to_string(),
                    due_on: date
                        .checked_add_days(Days::new(u64::from(interval)))
                        .unwrap_or(*date)
                        .format("%Y-%m-%d")
                        .to_string(),
                    count: rows.len() as u32,
                    notes,
                }
            })
            .collect()
    }
}

/// Bin-pack Ready candidates by note into 7 buckets, largest note
/// first into the least-loaded bucket. A note bigger than
/// `ceil(total / 7)` splits into consecutive line-order chunks. Ties
/// break by path, then bucket index. Bucket `k` (1-based) lands on
/// `today − 7 + k`.
fn bucket_ready<'a>(
    candidates: &[&'a RowCtx],
    today: NaiveDate,
) -> ReadyBuckets<'a> {
    let dates: Vec<NaiveDate> = (1..=7)
        .map(|index| {
            today
                .checked_sub_days(Days::new(7 - index))
                .unwrap_or(today)
        })
        .collect();
    let mut members: Vec<Vec<&RowCtx>> = vec![Vec::new(); 7];
    if candidates.is_empty() {
        return ReadyBuckets { dates, members };
    }

    let total = candidates.len();
    let cap = total.div_ceil(7).max(1);

    let mut by_note: BTreeMap<&str, Vec<&&RowCtx>> = BTreeMap::new();
    for candidate in candidates {
        by_note
            .entry(candidate.task.path.as_str())
            .or_default()
            .push(candidate);
    }
    // Units are single notes, or consecutive line-order chunks of an
    // oversized note.
    let mut units: Vec<Vec<&RowCtx>> = Vec::new();
    for (path, mut rows) in by_note {
        let _ = path;
        rows.sort_by_key(|row| row.task.line);
        for chunk in rows.chunks(cap) {
            units.push(chunk.iter().copied().copied().collect());
        }
    }
    // Largest first; ties by path, then first line.
    units.sort_by(|a, b| {
        b.len().cmp(&a.len()).then(
            (a[0].task.path.clone(), a[0].task.line)
                .cmp(&(b[0].task.path.clone(), b[0].task.line)),
        )
    });

    let mut loads = [0_usize; 7];
    for unit in units {
        let mut best = 0;
        for index in 1..7 {
            if loads[index] < loads[best] {
                best = index;
            }
        }
        loads[best] += unit.len();
        members[best].extend(unit);
    }
    for bucket in &mut members {
        bucket.sort_by(|a, b| {
            (a.task.path.clone(), a.task.line)
                .cmp(&(b.task.path.clone(), b.task.line))
        });
    }
    ReadyBuckets { dates, members }
}

/// Re-parse every changed line with both Rust parsers and abort the
/// whole seed — no writes — when any Tasks field differs.
fn check_invariance(changes: &[SeedChange]) -> Result<(), SeedError> {
    let mut mismatched = Vec::new();
    for change in changes {
        let before_fields = tasks_fingerprint(&change.old_line);
        let after_fields = tasks_fingerprint(&change.new_line);
        let fields_equal = match (before_fields, after_fields) {
            (Some(before), Some(after)) => before == after,
            _ => false,
        };
        let (before_body, before_block) = metadata_inputs(&change.old_line);
        let (after_body, after_block) = metadata_inputs(&change.new_line);
        let metadata_equal =
            task_metadata(&before_body, before_block.as_deref())
                == task_metadata(&after_body, after_block.as_deref());
        if !fields_equal || !metadata_equal {
            mismatched.push(format!("{}:{}", change.path, change.line));
        }
    }
    if mismatched.is_empty() {
        Ok(())
    } else {
        mismatched.sort();
        mismatched.dedup();
        Err(SeedError::InvarianceAborted { lines: mismatched })
    }
}

/// The `(body, block_id)` inputs for the hooks parser: the task body
/// after the checkbox, with the block ID passed separately exactly as
/// the hooks call it.
fn metadata_inputs(line: &str) -> (String, Option<String>) {
    let body = strip_checkbox(line);
    let block_id = task_block_id(&body).map(str::to_string);
    (body, block_id)
}

fn strip_checkbox(line: &str) -> String {
    let trimmed = line.trim_start();
    let after_marker = ["- ", "* ", "+ "]
        .iter()
        .find_map(|marker| trimmed.strip_prefix(marker))
        .unwrap_or(trimmed);
    if let Some(rest) = after_marker.strip_prefix('[') {
        match rest.find(']') {
            Some(end) => rest[end + 1..].trim_start().to_string(),
            None => after_marker.to_string(),
        }
    } else if after_marker.bytes().take_while(u8::is_ascii_digit).count() > 0
        && let Some(dot) = after_marker.find(". ")
    {
        after_marker[dot + 2..].to_string()
    } else {
        after_marker.to_string()
    }
}

fn task_block_id(body: &str) -> Option<&str> {
    let trimmed = body.trim_end();
    let caret = trimmed.rfind(" ^")? + 1;
    let id = trimmed[caret + 1..].trim();
    (!id.is_empty()
        && id.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
        }))
    .then_some(&trimmed[caret + 1..])
}

/// Re-read every touched file and refuse — no writes — when one
/// changed since the scan; otherwise rewrite each file through a temp
/// file plus rename.
fn write_changes(
    bob_dir: &Path,
    snapshot: &Snapshot,
    changes: &[SeedChange],
) -> Result<(), SeedError> {
    let mut by_file: BTreeMap<&str, Vec<&SeedChange>> = BTreeMap::new();
    for change in changes {
        by_file
            .entry(change.path.as_str())
            .or_default()
            .push(change);
    }

    let mut fresh_contents: HashMap<String, String> = HashMap::new();
    for path in by_file.keys() {
        let absolute = bob_dir.join(path);
        let current = fs::read_to_string(&absolute).map_err(|error| {
            SeedError::WriteFailed {
                path: path.to_string(),
                message: format!("read {}: {error}", absolute.display()),
                written: Vec::new(),
            }
        })?;
        let scanned = snapshot.file_contents.get(*path);
        if scanned.is_some_and(|scanned| *scanned != current) {
            return Err(SeedError::ConcurrentChange {
                path: path.to_string(),
            });
        }
        fresh_contents.insert(path.to_string(), current);
    }

    let mut written = Vec::new();
    for (path, file_changes) in &by_file {
        let contents = fresh_contents
            .get(*path)
            .expect("touched file was just read");
        let mut lines: Vec<&str> = contents.lines().collect();
        // A trailing newline is preserved by re-joining below.
        let ends_with_newline = contents.ends_with('\n') || contents.is_empty();
        for change in file_changes.iter() {
            let index = (change.line as usize).saturating_sub(1);
            let Some(current) = lines.get(index) else {
                return Err(SeedError::WriteFailed {
                    path: path.to_string(),
                    message: format!("{path}:{} no longer exists", change.line),
                    written: written.clone(),
                });
            };
            if *current != change.old_line {
                return Err(SeedError::WriteFailed {
                    path: path.to_string(),
                    message: format!(
                        "{path}:{} changed since the scan",
                        change.line
                    ),
                    written: written.clone(),
                });
            }
            lines[index] = &change.new_line;
        }
        let mut output = lines.join("\n");
        if ends_with_newline && !output.is_empty() {
            output.push('\n');
        }
        let absolute = bob_dir.join(path);
        write_atomically(&absolute, &output).map_err(|error| {
            SeedError::WriteFailed {
                path: path.to_string(),
                message: error,
                written: written.clone(),
            }
        })?;
        written.push(path.to_string());
    }
    Ok(())
}

fn write_atomically(path: &Path, contents: &str) -> Result<(), String> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("no file name in {}", path.display()))?;
    let temp =
        path.with_file_name(format!(".{file_name}.{}.tmp", std::process::id()));
    let _ = fs::remove_file(&temp);
    fs::write(&temp, contents)
        .map_err(|error| format!("write {}: {error}", temp.display()))?;
    fs::rename(&temp, path).map_err(|error| {
        let _ = fs::remove_file(&temp);
        format!("rename {}: {error}", path.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn test_date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 8).expect("valid test date")
    }

    fn test_row(path: &str, line: u32) -> RowCtx {
        RowCtx {
            task: crate::native::dataview::RichTask {
                path: path.to_string(),
                line,
                status_symbol: " ".to_string(),
                status_type: "TODO".to_string(),
                original_markdown: format!(
                    "- [ ] #task Task {line} [created:: 2026-09-01]"
                ),
                text: format!("Task {line}"),
                created: NaiveDate::from_ymd_opt(2026, 9, 1),
                scheduled: None,
                is_recurring: false,
                is_blocked: false,
                tags: Vec::new(),
                block_id: None,
            },
            note_refresh_raw: None,
            is_daily_note: false,
            is_today: false,
        }
    }

    #[test]
    fn oversized_notes_split_into_consecutive_chunks() {
        // Eight tasks in one note: ceil(8 / 7) is 2, so the note
        // splits into four consecutive pairs spread over four buckets.
        let rows: Vec<RowCtx> =
            (1..=8).map(|line| test_row("big.md", line)).collect();
        let refs: Vec<&RowCtx> = rows.iter().collect();
        let buckets = bucket_ready(&refs, test_date());
        let counts: Vec<usize> = buckets.members.iter().map(Vec::len).collect();
        assert_eq!(counts, vec![2, 2, 2, 2, 0, 0, 0]);
        let first: Vec<u32> =
            buckets.members[0].iter().map(|row| row.task.line).collect();
        assert_eq!(first, vec![1, 2]);
        // Bucket k lands on today − 7 + k.
        assert_eq!(
            buckets.dates[0].format("%Y-%m-%d").to_string(),
            "2026-10-02"
        );
        assert_eq!(
            buckets.dates[6].format("%Y-%m-%d").to_string(),
            "2026-10-08"
        );
    }

    #[test]
    fn invariance_abort_lists_a_line_hiding_a_field() {
        // A misplaced, malformed fresh hides `created` from both Rust
        // parsers; repairing the placement would change the parsed
        // fields, so the guard trips.
        let old_line =
            "- [ ] #task Tricky [created:: 2026-09-01] [fresh:: not-a-date]";
        let new_line =
            "- [ ] #task Tricky [fresh:: 2026-10-08] [created:: 2026-09-01]";
        let changes = vec![SeedChange {
            path: "c.md".to_string(),
            line: 1,
            old_line: old_line.to_string(),
            new_line: new_line.to_string(),
            kind: SeedKind::Ready,
        }];
        let error = check_invariance(&changes).expect_err("must abort");
        match error {
            SeedError::InvarianceAborted { lines } => {
                assert_eq!(lines, vec!["c.md:1".to_string()]);
            }
            other => panic!("wrong error: {}", other.message()),
        }
    }

    #[test]
    fn canonical_restamps_pass_invariance() {
        let old_line =
            "- [ ] #task Buy milk [fresh:: 2026-10-01] [created:: 2026-09-29]";
        let new_line =
            "- [ ] #task Buy milk [fresh:: 2026-10-08] [created:: 2026-09-29]";
        let changes = vec![SeedChange {
            path: "a.md".to_string(),
            line: 1,
            old_line: old_line.to_string(),
            new_line: new_line.to_string(),
            kind: SeedKind::Ready,
        }];
        check_invariance(&changes).expect("canonical restamp is safe");
    }

    #[test]
    fn write_changes_refuses_a_file_changed_since_scan() {
        let dir = std::env::temp_dir()
            .join(format!("bob-cli-seed-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create seed test dir");
        let scanned = "- [ ] #task Buy milk [created:: 2026-09-29]\n";
        fs::write(dir.join("a.md"), "CHANGED\n").expect("write changed");
        let snapshot = Snapshot {
            today: test_date(),
            weekday: "Thu".to_string(),
            config: FreshnessConfig::default(),
            ready: Vec::new(),
            pending: Vec::new(),
            next: Vec::new(),
            trackers: Vec::new(),
            open: Vec::new(),
            all: Vec::new(),
            today_warnings: Vec::new(),
            file_contents: HashMap::from([(
                "a.md".to_string(),
                scanned.to_string(),
            )]),
        };
        let changes = vec![SeedChange {
            path: "a.md".to_string(),
            line: 1,
            old_line: scanned.trim_end().to_string(),
            new_line: "- [ ] #task Buy milk [fresh:: 2026-10-08] [created:: 2026-09-29]"
                .to_string(),
            kind: SeedKind::Ready,
        }];
        let error =
            write_changes(&dir, &snapshot, &changes).expect_err("must refuse");
        match error {
            SeedError::ConcurrentChange { path } => {
                assert_eq!(path, "a.md");
            }
            other => panic!("wrong error: {}", other.message()),
        }
        assert_eq!(
            fs::read_to_string(dir.join("a.md")).expect("read back"),
            "CHANGED\n",
            "refusal must not write"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}

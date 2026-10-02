//! Linkable-task discovery for the `:` picker.
//!
//! A read-only scanner that lists Ready (` `), Blocked (`?`), Next (`*`),
//! and In Progress (`/`) tasks from routable inbox, area, and project
//! notes, annotated with the group, the queued Pomodoro (if any),
//! block-ID suggestions for ID-less tasks, schedule pull-forward flags,
//! and a deterministic tiered fuzzy ranker. The completion-contract phase
//! wires this into `capture-complete`'s `task_link` context.

use std::{cmp::Reverse, collections::HashSet, fs, io, path::Path};

use chrono::NaiveDate;
use serde::Serialize;

use super::{
    capture_active_tasks::{self, ActiveTaskPomodoro},
    capture_block_ids,
    capture_targets::{self, CaptureTargetKind},
    capture_tasks, collect_done, env as bob_env, note_tasks, pomodoro,
    task_fields,
};

/// Whether a task status can be linked into a Pomodoro: Ready, Blocked,
/// Next, or In Progress. Shared by the link resolver, the link-only
/// block-ID candidates, and this scanner so the three can never drift
/// apart.
pub(crate) fn is_linkable_status(symbol: char) -> bool {
    matches!(symbol, ' ' | '?' | '*' | '/')
}

/// Which bucket a linkable task lists under in the `:` picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LinkTaskGroup {
    Queued,
    InProgress,
    Next,
    Note,
}

/// One linkable task: an open task with a linkable status in a routable
/// inbox, area, or non-terminal project note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct LinkTask {
    pub(crate) route: String,
    pub(crate) note_kind: CaptureTargetKind,
    pub(crate) block_id: Option<String>,
    pub(crate) block_id_suggestions: Vec<String>,
    pub(crate) status_symbol: char,
    pub(crate) status_name: String,
    pub(crate) status_type: &'static str,
    pub(crate) text: String,
    pub(crate) section: Option<String>,
    pub(crate) depth: usize,
    pub(crate) line: usize,
    #[serde(rename = "ref")]
    pub(crate) task_ref: String,
    pub(crate) scheduled: Option<String>,
    /// `true` when linking would retire a future `scheduled` field,
    /// omitted when `false`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) pulls_forward: bool,
    pub(crate) pomodoro: Option<ActiveTaskPomodoro>,
    pub(crate) group: LinkTaskGroup,
}

fn is_false(value: &bool) -> bool {
    !value
}

impl LinkTask {
    /// `@route:id`, the string a `:` accept inserts. ID-less tasks have
    /// no linkable spelling, so clients must never insert their empty
    /// replacement.
    pub(crate) fn replacement(&self) -> String {
        match &self.block_id {
            Some(id) => format!("@{}:{id}", self.route),
            None => String::new(),
        }
    }
}

/// Discovery output: canonically ordered candidates plus bounded
/// warnings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LinkTaskResult {
    pub(crate) tasks: Vec<LinkTask>,
    pub(crate) warnings: Vec<String>,
}

/// Scan every capture target for linkable tasks, using the capture clock
/// (`BOB_NOW`, else the local date) for pull-forward decisions.
pub(crate) fn discover(bob_dir: &Path) -> LinkTaskResult {
    let today = bob_env::current_datetime().date();
    discover_at(bob_dir, &pomodoro::day_file_for(bob_dir), today)
}

/// [`discover`] with an explicit daily-note path and date, so tests can
/// pass a temp ledger and a pinned today.
pub(crate) fn discover_at(
    bob_dir: &Path,
    day_file: &Path,
    today: NaiveDate,
) -> LinkTaskResult {
    let mut warnings = Vec::new();
    let report = capture_targets::scan_capture_targets(bob_dir);
    for issue in &report.issues {
        warnings.push(capture_active_tasks::bounded_warning(issue.display()));
    }
    let settings = note_tasks::read_settings(bob_dir);
    let mut notes = Vec::new();
    for target in &report.targets {
        let relative = &target.relative_path;
        match fs::read_to_string(bob_dir.join(relative)) {
            Ok(contents) => notes.push((target, contents)),
            Err(error)
                if target.kind == CaptureTargetKind::Inbox
                    && error.kind() == io::ErrorKind::NotFound =>
            {
                // A missing inbox file is silent.
            }
            Err(error) => {
                warnings.push(capture_active_tasks::bounded_warning(format!(
                    "failed to read {relative}: {error}"
                )));
            }
        }
    }
    let ledger = capture_active_tasks::read_ledger(day_file, &mut warnings);

    let mut ordered: Vec<(LinkTask, (u8, usize, usize))> = Vec::new();
    for (target_index, (target, contents)) in notes.iter().enumerate() {
        let scan = note_tasks::scan(contents, &settings);
        let used_ids = collect_done::block_ids_in_markdown(contents);
        // One used-ID set per note: rebuilding it per ID-less task would
        // re-hash the same list hundreds of times on large notes.
        let used: HashSet<&str> = used_ids.iter().map(String::as_str).collect();
        let lines: Vec<&str> = contents.lines().collect();
        for task in scan
            .open_tasks()
            .filter(|task| is_linkable_status(task.status_symbol))
        {
            let raw_line =
                lines.get(task.line_index).copied().unwrap_or_default();
            let key = task
                .block_id
                .as_ref()
                .map(|id| (target.route.clone(), id.clone()));
            let position = key.as_ref().and_then(|key| ledger.position(key));
            let pomodoro =
                key.as_ref().and_then(|key| ledger.owners.get(key).cloned());
            let group = if position.is_some() {
                LinkTaskGroup::Queued
            } else if task.status_symbol == '/' {
                LinkTaskGroup::InProgress
            } else if task.status_symbol == '*' {
                LinkTaskGroup::Next
            } else {
                LinkTaskGroup::Note
            };
            let sort_key = match group {
                LinkTaskGroup::Queued => {
                    let (entry, link) =
                        position.expect("queued tasks have a position");
                    (0, entry, link)
                }
                LinkTaskGroup::InProgress => (1, 0, 0),
                LinkTaskGroup::Next => (2, 0, 0),
                LinkTaskGroup::Note => (3, target_index, task.line_index),
            };
            // One field pass per task line feeds both the displayed
            // date and the pull-forward flag.
            let (scheduled, pulls_forward) = scheduled_facts(raw_line, today);
            ordered.push((
                LinkTask {
                    route: target.route.clone(),
                    note_kind: target.kind,
                    block_id: task.block_id.clone(),
                    block_id_suggestions: if task.block_id.is_some() {
                        Vec::new()
                    } else {
                        capture_block_ids::suggest_ids_with_used(
                            &task.description,
                            ':',
                            &used,
                        )
                    },
                    status_symbol: task.status_symbol,
                    status_name: task.status_name.clone(),
                    status_type: capture_tasks::status_type_label(
                        task.status_type,
                    ),
                    text: task.description.clone(),
                    section: task.section.clone(),
                    depth: capture_tasks::indentation_depth(&task.indentation),
                    line: task.line_index + 1,
                    task_ref: task.task_ref(),
                    scheduled,
                    pulls_forward,
                    pomodoro,
                    group,
                },
                sort_key,
            ));
        }
    }
    // Stable sorts compose: order the middle groups by route then
    // line first (a constant key elsewhere), then by the group key.
    ordered.sort_by(|left, right| {
        secondary_order(left).cmp(&secondary_order(right))
    });
    ordered.sort_by_key(|(_, key)| *key);
    LinkTaskResult {
        tasks: ordered.into_iter().map(|(task, _)| task).collect(),
        warnings,
    }
}

/// Route then line within the In Progress and Next groups; a constant
/// elsewhere so the group sort keeps queued ledger order and note
/// document order.
fn secondary_order(
    (task, key): &(LinkTask, (u8, usize, usize)),
) -> (&str, usize) {
    if matches!(key.0, 1..=2) {
        (task.route.as_str(), task.line)
    } else {
        ("", 0)
    }
}

/// The schedule facts for one raw task line from a single field pass:
/// the first strict `YYYY-MM-DD` scheduled value in line order (or `None`
/// when the line carries no valid one), and whether linking would retire a
/// future date — exactly one recognized `scheduled` field, strictly valid,
/// and later than `today`, matching
/// `capture_task_toggle::find_single_future_scheduled_field`.
fn scheduled_facts(raw_line: &str, today: NaiveDate) -> (Option<String>, bool) {
    let fields = task_fields::inline_fields(raw_line, "scheduled");
    let scheduled = fields.iter().find_map(|field| {
        task_fields::parse_strict_calendar_date(&field.value)
            .map(|_| field.value.clone())
    });
    let pulls_forward = match fields.as_slice() {
        [only] => task_fields::parse_strict_calendar_date(&only.value)
            .is_some_and(|date| date > today),
        _ => false,
    };
    (scheduled, pulls_forward)
}

/// Rank candidates for a `:` query: every whitespace-separated term must
/// match, and tasks order by the sum of their per-term tiers,
/// descending, then by canonical order. The sort is stable, so ties and
/// an empty query keep the canonical order.
pub(crate) fn rank<'a>(
    tasks: &'a [LinkTask],
    query: &str,
) -> Vec<&'a LinkTask> {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|term| term.to_lowercase())
        .collect();
    if terms.is_empty() {
        return tasks.iter().collect();
    }
    let mut scored = Vec::new();
    for task in tasks {
        let mut total = 0u32;
        let mut matched = true;
        for term in &terms {
            match term_tier(task, term) {
                Some(tier) => total += tier,
                None => {
                    matched = false;
                    break;
                }
            }
        }
        if matched {
            scored.push((task, total));
        }
    }
    scored.sort_by_key(|(_, score)| Reverse(*score));
    scored.into_iter().map(|(task, _)| task).collect()
}

/// A term's best tier over the searchable fields, or `None` when it
/// matches nothing. Tiers, best first: 3 for a field prefix, 2 for a
/// word prefix (the preceding character is not alphanumeric), 1 for a
/// substring, and 0 for an in-order subsequence.
fn term_tier(task: &LinkTask, term: &str) -> Option<u32> {
    let mut best = None;
    let mut consider = |field: &str| {
        if let Some(tier) = field_tier(&field.to_lowercase(), term)
            && best.is_none_or(|current| tier > current)
        {
            best = Some(tier);
        }
    };
    if let Some(block_id) = &task.block_id {
        consider(&format!("{}:{block_id}", task.route));
        consider(block_id);
    }
    consider(&task.text);
    consider(&task.route);
    if let Some(section) = &task.section {
        consider(section);
    }
    if let Some(name) =
        task.pomodoro.as_ref().and_then(|entry| entry.name.as_ref())
    {
        consider(name);
    }
    best
}

fn field_tier(field: &str, term: &str) -> Option<u32> {
    if field.starts_with(term) {
        return Some(3);
    }
    for (index, _) in field.char_indices() {
        if index > 0
            && field[index..].starts_with(term)
            && field[..index]
                .chars()
                .next_back()
                .is_some_and(|cell| !cell.is_alphanumeric())
        {
            return Some(2);
        }
    }
    if field.contains(term) {
        return Some(1);
    }
    if is_subsequence(field, term) {
        return Some(0);
    }
    None
}

/// Whether every character of `term` appears in `field` in order. Both
/// sides are already lowercased.
fn is_subsequence(field: &str, term: &str) -> bool {
    let mut wanted = term.chars();
    let mut current = wanted.next();
    for cell in field.chars() {
        if Some(cell) == current {
            current = wanted.next();
            if current.is_none() {
                return true;
            }
        }
    }
    current.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|error| {
                panic!("create parent {}: {error}", parent.display())
            });
        }
        fs::write(path, contents).unwrap_or_else(|error| {
            panic!("write {}: {error}", path.display())
        });
    }

    fn write_settings(root: &Path) {
        write_file(
            &root.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
            r##"{
              "globalFilter": "#task",
              "statusSettings": {
                "coreStatuses": [
                  {"symbol":" ","name":"Todo","type":"TODO"},
                  {"symbol":"x","name":"Done","type":"DONE"},
                  {"symbol":"/","name":"In Progress","type":"IN_PROGRESS"},
                  {"symbol":"*","name":"Next","type":"ON_HOLD"},
                  {"symbol":"-","name":"Canceled","type":"CANCELLED"}
                ],
                "customStatuses": [
                  {"symbol":"?","name":"Blocked","type":"ON_HOLD"}
                ]
              }
            }"##,
        );
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, 30).expect("valid date")
    }

    /// A temp ledger outside the vault root, so target enumeration never
    /// picks the daily note itself up as a task note.
    fn day_ledger(root: &Path, contents: &str) -> PathBuf {
        let day_file = root.join("ledger/day.md");
        write_file(&day_file, contents);
        day_file
    }

    fn discover_with_ledger(root: &Path, contents: &str) -> LinkTaskResult {
        let day_file = day_ledger(root, contents);
        discover_at(root, &day_file, today())
    }

    fn write_worked_example(root: &Path) {
        write_settings(root);
        write_file(
            &root.join("mac_inbox.md"),
            "---\n\
             type: [[area]]\n\
             ---\n\
             - [ ] #task Call the bank [created::2026-09-29]\n",
        );
        write_file(
            &root.join("health.md"),
            "---\n\
             type: [[area]]\n\
             ---\n\
             ## Errands\n\
             - [?] #task Book dentist [scheduled::2026-10-03]\n",
        );
        write_file(
            &root.join("bob.md"),
            "---\n\
             type: [[project]]\n\
             status: wip\n\
             ---\n\
             - [ ] #task Polish capture picker ^polish\n\
             \t- [ ] #task Tune fuzzy weights\n",
        );
        write_file(
            &root.join("sase.md"),
            "---\n\
             type: [[project]]\n\
             status: wip\n\
             ---\n\
             ## Bugs\n\
             - [*] #task Fix deep bug ^deep-fix\n\
             - [ ] #task Fix flaky gkeep test\n\
             - [x] #task Old fix ^old-fix\n\
             ## Writing\n\
             - [/] #task Draft outline ^outline\n\
             - [ ] #task Ship blog post #now ^blog\n\
             - [-] #task Dropped idea\n",
        );
        write_file(
            &root.join("archive.md"),
            "---\n\
             type: [[project]]\n\
             status: done\n\
             ---\n\
             - [ ] #task Leftover ^leftover\n",
        );
        write_file(&root.join("scratch.md"), "- [ ] #task Loose end ^loose\n");
    }

    fn worked_example() -> (TempDir, LinkTaskResult) {
        let temp = TempDir::new("bob-cli-link-tasks-worked");
        write_worked_example(temp.path());
        let result = discover_with_ledger(
            temp.path(),
            "## Pomodoros\n\
             - [ ] () — BUGS\n\
             \t- [[sase#^deep-fix]]\n",
        );
        (temp, result)
    }

    fn replacements(result: &LinkTaskResult) -> Vec<String> {
        result.tasks.iter().map(|task| task.replacement()).collect()
    }

    #[test]
    fn worked_example_lists_eight_rows_in_canonical_order() {
        let (_temp, result) = worked_example();
        assert!(result.warnings.is_empty());
        assert_eq!(
            replacements(&result),
            vec![
                "@sase:deep-fix",
                "@sase:outline",
                "",
                "",
                "@bob:polish",
                "",
                "",
                "@sase:blog",
            ]
        );
        assert_eq!(
            result
                .tasks
                .iter()
                .map(|task| (task.route.as_str(), task.group))
                .collect::<Vec<_>>(),
            vec![
                ("sase", LinkTaskGroup::Queued),
                ("sase", LinkTaskGroup::InProgress),
                ("mac_inbox", LinkTaskGroup::Note),
                ("health", LinkTaskGroup::Note),
                ("bob", LinkTaskGroup::Note),
                ("bob", LinkTaskGroup::Note),
                ("sase", LinkTaskGroup::Note),
                ("sase", LinkTaskGroup::Note),
            ]
        );

        let queued = &result.tasks[0];
        assert_eq!(queued.note_kind, CaptureTargetKind::Project);
        assert_eq!(queued.block_id.as_deref(), Some("deep-fix"));
        assert!(queued.block_id_suggestions.is_empty());
        assert_eq!(queued.status_symbol, '*');
        assert_eq!(queued.status_name, "Next");
        assert_eq!(queued.status_type, "ON_HOLD");
        assert_eq!(queued.text, "Fix deep bug");
        assert_eq!(queued.section.as_deref(), Some("Bugs"));
        assert_eq!(queued.depth, 0);
        assert_eq!(queued.line, 6);
        assert!(queued.task_ref.starts_with("6:"));
        assert_eq!(queued.scheduled, None);
        assert!(!queued.pulls_forward);
        let pomodoro = queued.pomodoro.as_ref().expect("queued");
        assert_eq!(pomodoro.name.as_deref(), Some("BUGS"));
        assert_eq!(pomodoro.line, 2);

        let wip = &result.tasks[1];
        assert_eq!(wip.block_id.as_deref(), Some("outline"));
        assert!(wip.block_id_suggestions.is_empty());
        assert_eq!(wip.status_symbol, '/');
        assert_eq!(wip.status_name, "In Progress");
        assert_eq!(wip.status_type, "IN_PROGRESS");
        assert_eq!(wip.text, "Draft outline");
        assert_eq!(wip.section.as_deref(), Some("Writing"));
        assert_eq!(wip.depth, 0);
        assert_eq!(wip.line, 10);
        assert_eq!(wip.scheduled, None);
        assert!(!wip.pulls_forward);
        assert!(wip.pomodoro.is_none());

        let inbox = &result.tasks[2];
        assert_eq!(inbox.route, "mac_inbox");
        assert_eq!(inbox.note_kind, CaptureTargetKind::Inbox);
        assert_eq!(inbox.block_id, None);
        assert_eq!(inbox.block_id_suggestions, vec!["call-bank".to_string()]);
        assert_eq!(inbox.status_symbol, ' ');
        assert_eq!(inbox.text, "Call the bank");
        assert_eq!(inbox.section, None);
        assert_eq!(inbox.depth, 0);
        assert_eq!(inbox.line, 4);
        assert_eq!(inbox.scheduled, None);
        assert!(!inbox.pulls_forward);
        assert!(inbox.pomodoro.is_none());

        let dentist = &result.tasks[3];
        assert_eq!(dentist.route, "health");
        assert_eq!(dentist.note_kind, CaptureTargetKind::Area);
        assert_eq!(dentist.block_id, None);
        assert_eq!(
            dentist.block_id_suggestions,
            vec!["book-dentist".to_string()]
        );
        assert_eq!(dentist.status_symbol, '?');
        assert_eq!(dentist.status_name, "Blocked");
        assert_eq!(dentist.status_type, "ON_HOLD");
        assert_eq!(dentist.text, "Book dentist");
        assert_eq!(dentist.section.as_deref(), Some("Errands"));
        assert_eq!(dentist.line, 5);
        assert_eq!(dentist.scheduled.as_deref(), Some("2026-10-03"));
        assert!(dentist.pulls_forward);

        let polish = &result.tasks[4];
        assert_eq!(polish.route, "bob");
        assert_eq!(polish.note_kind, CaptureTargetKind::Project);
        assert_eq!(polish.block_id.as_deref(), Some("polish"));
        assert!(polish.block_id_suggestions.is_empty());
        assert_eq!(polish.text, "Polish capture picker");
        assert_eq!(polish.section, None);
        assert_eq!(polish.depth, 0);
        assert_eq!(polish.line, 5);
        assert_eq!(polish.scheduled, None);
        assert!(!polish.pulls_forward);

        let nested = &result.tasks[5];
        assert_eq!(nested.route, "bob");
        assert_eq!(nested.block_id, None);
        assert_eq!(
            nested.block_id_suggestions,
            vec!["tune-fuzzy-weights".to_string()]
        );
        assert_eq!(nested.text, "Tune fuzzy weights");
        assert_eq!(nested.depth, 1);
        assert_eq!(nested.line, 6);
        assert!(!nested.pulls_forward);

        let flaky = &result.tasks[6];
        assert_eq!(flaky.route, "sase");
        assert_eq!(flaky.block_id, None);
        assert_eq!(
            flaky.block_id_suggestions,
            vec![
                "fix-flaky-gkeep".to_string(),
                "flaky-gkeep-test".to_string()
            ]
        );
        assert_eq!(flaky.text, "Fix flaky gkeep test");
        assert_eq!(flaky.section.as_deref(), Some("Bugs"));
        assert_eq!(flaky.depth, 0);
        assert_eq!(flaky.line, 7);
        assert_eq!(flaky.scheduled, None);
        assert!(!flaky.pulls_forward);

        let bet = &result.tasks[7];
        assert_eq!(bet.block_id.as_deref(), Some("blog"));
        assert!(bet.block_id_suggestions.is_empty());
        assert_eq!(bet.status_symbol, ' ');
        assert_eq!(bet.status_name, "Todo");
        assert_eq!(bet.status_type, "TODO");
        assert_eq!(bet.text, "Ship blog post #now");
        assert_eq!(bet.section.as_deref(), Some("Writing"));
        assert_eq!(bet.line, 11);
        assert_eq!(bet.group, LinkTaskGroup::Note);
        assert_eq!(bet.scheduled, None);
        assert!(!bet.pulls_forward);
        assert!(bet.pomodoro.is_none());
    }

    #[test]
    fn excludes_closed_unknown_terminal_untyped_and_unroutable() {
        let temp = TempDir::new("bob-cli-link-tasks-excluded");
        write_settings(temp.path());
        write_file(
            &temp.path().join("ok.md"),
            "---\ntype: [[area]]\n---\n- [ ] #task Keep ^keep\n",
        );
        write_file(
            &temp.path().join("gone.md"),
            "---\n\
             type: [[project]]\n\
             status: done\n\
             ---\n\
             - [ ] #task Terminal ^terminal\n",
        );
        write_file(
            &temp.path().join("old.md"),
            "---\n\
             type: [[project]]\n\
             status: canceled\n\
             ---\n\
             - [ ] #task Canceled project ^canceled\n",
        );
        write_file(
            &temp.path().join("weird.md"),
            "---\n\
             type: [[area]]\n\
             ---\n\
             - [Q] #task Unknown symbol ^weird\n\
             - [x] #task Done ^done\n\
             - [-] #task Dropped ^dropped\n",
        );
        write_file(
            &temp.path().join("plain.md"),
            "- [ ] #task Untyped ^untyped\n",
        );
        write_file(
            &temp.path().join("not a route.md"),
            "---\ntype: [[area]]\n---\n- [ ] #task Spaced ^spaced\n",
        );
        write_file(
            &temp.path().join("sub/inner.md"),
            "---\n\
             type: [[area]]\n\
             ---\n\
             - [ ] #task Nested dir ^nested-dir\n",
        );
        let result = discover_with_ledger(
            temp.path(),
            "## Pomodoros\n- [ ] () — BUGS\n",
        );
        assert!(result.warnings.is_empty());
        assert_eq!(replacements(&result), vec!["@ok:keep"]);
    }

    #[test]
    fn suggestions_avoid_used_ids_including_non_task_anchors() {
        let temp = TempDir::new("bob-cli-link-tasks-suggest");
        write_settings(temp.path());
        write_file(
            &temp.path().join("tools.md"),
            "---\n\
             type: [[area]]\n\
             ---\n\
             - [ ] #task Bright ^bright\n\
             - [ ] #task Bright\n\
             - [ ] #task Tool\n\
             para ^tool\n",
        );
        let result = discover_with_ledger(
            temp.path(),
            "## Pomodoros\n- [ ] () — BUGS\n",
        );
        assert!(result.warnings.is_empty());
        assert_eq!(result.tasks.len(), 3);
        assert!(result.tasks[0].block_id_suggestions.is_empty());
        assert_eq!(
            result.tasks[1].block_id_suggestions,
            vec!["bright-2".to_string()]
        );
        assert_eq!(
            result.tasks[2].block_id_suggestions,
            vec!["tool-2".to_string()]
        );
    }

    #[test]
    fn pulls_forward_only_for_a_single_future_scheduled_field() {
        let temp = TempDir::new("bob-cli-link-tasks-scheduled");
        write_settings(temp.path());
        write_file(
            &temp.path().join("sched.md"),
            "---\n\
             type: [[area]]\n\
             ---\n\
             - [ ] #task Future [scheduled:: 2026-10-03] ^future\n\
             - [ ] #task Past [scheduled:: 2026-09-01] ^past\n\
             - [ ] #task Two [scheduled:: 2026-10-03] \
             [scheduled:: 2026-10-04] ^two\n\
             - [ ] #task Bad [scheduled:: soon] ^bad\n",
        );
        let result = discover_with_ledger(
            temp.path(),
            "## Pomodoros\n- [ ] () — BUGS\n",
        );
        assert!(result.warnings.is_empty());
        let by_id = |id: &str| {
            result
                .tasks
                .iter()
                .find(|task| task.block_id.as_deref() == Some(id))
                .unwrap_or_else(|| panic!("missing task {id}"))
        };
        let future = by_id("future");
        assert_eq!(future.scheduled.as_deref(), Some("2026-10-03"));
        assert!(future.pulls_forward);
        let past = by_id("past");
        assert_eq!(past.scheduled.as_deref(), Some("2026-09-01"));
        assert!(!past.pulls_forward);
        let two = by_id("two");
        assert_eq!(two.scheduled.as_deref(), Some("2026-10-03"));
        assert!(!two.pulls_forward);
        let bad = by_id("bad");
        assert_eq!(bad.scheduled, None);
        assert!(!bad.pulls_forward);
    }

    #[test]
    fn warns_when_the_day_file_is_missing_but_lists_tasks() {
        let temp = TempDir::new("bob-cli-link-tasks-no-day");
        write_worked_example(temp.path());
        let missing = temp.path().join("2010/20100101.md");
        let result = discover_at(temp.path(), &missing, today());
        assert_eq!(result.tasks.len(), 8);
        assert!(result.tasks.iter().all(|task| task.pomodoro.is_none()));
        assert!(
            result
                .tasks
                .iter()
                .all(|task| task.group != LinkTaskGroup::Queued),
            "nothing is queued without a ledger"
        );
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("does not exist"));
    }

    #[test]
    fn warns_when_the_pomodoros_section_is_missing() {
        let temp = TempDir::new("bob-cli-link-tasks-no-section");
        write_worked_example(temp.path());
        let result = discover_with_ledger(
            temp.path(),
            "# Just a note\n- [*] Deep #task ^deep-fix\n",
        );
        assert_eq!(result.tasks.len(), 8);
        assert!(result.tasks.iter().all(|task| task.pomodoro.is_none()));
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("no Pomodoros section"));
    }

    #[test]
    fn warns_for_unreadable_notes_and_keeps_other_candidates() {
        let temp = TempDir::new("bob-cli-link-tasks-unreadable");
        write_settings(temp.path());
        write_file(
            &temp.path().join("sase.md"),
            "---\n\
             type: [[project]]\n\
             status: wip\n\
             ---\n\
             - [*] Deep #task ^deep-fix\n",
        );
        let locked = temp.path().join("locked.md");
        write_file(
            &locked,
            "---\n\
             type: [[area]]\n\
             ---\n\
             - [*] Locked #task ^locked\n",
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
                .expect("remove read permission");
        }
        let result = discover_with_ledger(
            temp.path(),
            "## Pomodoros\n- [ ] () — BUGS\n",
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o644))
                .expect("restore read permission");
        }
        assert!(
            result
                .tasks
                .iter()
                .any(|task| task.block_id == Some("deep-fix".to_string())),
            "a failed read still returns the other candidates"
        );
        if result
            .tasks
            .iter()
            .any(|task| task.block_id == Some("locked".to_string()))
        {
            // Running with privileges that ignore file permissions; the
            // candidate set is complete so no warning is required.
            return;
        }
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("locked.md"));
        assert!(result.warnings[0].contains("failed to read"));
    }

    #[test]
    fn ranker_matches_the_worked_example_queries() {
        let (_temp, result) = worked_example();
        let ranked = |query: &str| {
            rank(&result.tasks, query)
                .iter()
                .map(|task| task.replacement())
                .collect::<Vec<_>>()
        };
        assert_eq!(ranked(""), replacements(&result));
        assert_eq!(ranked("dee"), vec!["@sase:deep-fix"]);
        assert_eq!(ranked("bug"), vec!["@sase:deep-fix", ""]);
        assert_eq!(ranked("sase out"), vec!["@sase:outline"]);
        assert_eq!(ranked("dntst"), vec![""]);
        assert_eq!(ranked("sase:out"), vec!["@sase:outline"]);
        assert_eq!(ranked("sase:deep-fix="), Vec::<String>::new());
        // Every term must match: no task carries both terms.
        assert_eq!(ranked("sase health"), Vec::<String>::new());
        // Ties keep the canonical order.
        assert_eq!(
            ranked("sase"),
            vec!["@sase:deep-fix", "@sase:outline", "", "@sase:blog"]
        );
    }

    #[test]
    fn ranker_orders_by_tier_sum_then_canonical_order() {
        let first = LinkTask {
            route: "sase".to_string(),
            note_kind: CaptureTargetKind::Project,
            block_id: None,
            block_id_suggestions: Vec::new(),
            status_symbol: ' ',
            status_name: "Todo".to_string(),
            status_type: "TODO",
            text: "a sandwich".to_string(),
            section: None,
            depth: 0,
            line: 2,
            task_ref: "2:deadbeef".to_string(),
            scheduled: None,
            pulls_forward: false,
            pomodoro: None,
            group: LinkTaskGroup::Note,
        };
        let second = LinkTask {
            text: "sandwich".to_string(),
            line: 1,
            task_ref: "1:deadbeef".to_string(),
            ..first.clone()
        };
        let tasks = vec![first, second];
        // A field prefix (3) outranks a word prefix (2) even when the
        // weaker match comes first in canonical order.
        let ranked = rank(&tasks, "sand")
            .iter()
            .map(|task| task.text.clone())
            .collect::<Vec<_>>();
        assert_eq!(ranked, vec!["sandwich", "a sandwich"]);

        let apple = LinkTask {
            text: "apple banana".to_string(),
            ..tasks[1].clone()
        };
        let words = LinkTask {
            text: "an apple and a banana".to_string(),
            ..tasks[1].clone()
        };
        let tasks = vec![words, apple];
        // 3 + 2 outranks 2 + 2 across two AND terms.
        let ranked = rank(&tasks, "apple banana")
            .iter()
            .map(|task| task.text.clone())
            .collect::<Vec<_>>();
        assert_eq!(ranked, vec!["apple banana", "an apple and a banana"]);
    }

    #[test]
    fn ranker_passes_the_dependency_contract_dk_vectors() {
        // DK vectors from docs/task-dependencies.md §11.4: the Depends
        // on stage ports `rank`, so the contract pins them here.
        fn candidate(
            text: &str,
            route: &str,
            block_id: Option<&str>,
            section: Option<&str>,
        ) -> LinkTask {
            LinkTask {
                route: route.to_string(),
                note_kind: CaptureTargetKind::Project,
                block_id: block_id.map(str::to_string),
                block_id_suggestions: Vec::new(),
                status_symbol: ' ',
                status_name: "Todo".to_string(),
                status_type: "TODO",
                text: text.to_string(),
                section: section.map(str::to_string),
                depth: 0,
                line: 1,
                task_ref: "1:deadbeef".to_string(),
                scheduled: None,
                pulls_forward: false,
                pomodoro: None,
                group: LinkTaskGroup::Note,
            }
        }
        let tasks = vec![
            candidate(
                "File for unemployment",
                "cash",
                Some("unemployment"),
                Some("Money"),
            ),
            candidate("Call unemployment office", "cash", None, Some("Money")),
            candidate("Dispute North Face jacket", "cash", None, None),
            candidate(
                "Launch swarm to find hospital",
                "body",
                Some("hospital-swarm"),
                Some("Health"),
            ),
            candidate(
                "Run e2e on sase-8v",
                "sase_bug_bash",
                Some("e2e-sase-8v"),
                Some("Bugs"),
            ),
        ];
        let ranked = |query: &str| {
            rank(&tasks, query)
                .iter()
                .map(|task| task.text.clone())
                .collect::<Vec<_>>()
        };
        // DK1: a field prefix (3) outranks a word prefix (2).
        assert_eq!(
            ranked("unemp"),
            vec!["File for unemployment", "Call unemployment office",]
        );
        // DK2: `route:blockId` field prefix.
        assert_eq!(ranked("cash:un"), vec!["File for unemployment"]);
        // DK3: word prefix.
        assert_eq!(ranked("face"), vec!["Dispute North Face jacket"]);
        // DK4: substring inside `hospital`.
        assert_eq!(ranked("pit"), vec!["Launch swarm to find hospital"]);
        // DK5: in-order subsequence only; ties keep canonical order.
        assert_eq!(
            ranked("uof"),
            vec![
                "Call unemployment office",
                "Dispute North Face jacket",
                "Launch swarm to find hospital",
            ]
        );
        // DK6: block-id field prefix.
        assert_eq!(ranked("e2e"), vec!["Run e2e on sase-8v"]);
        // DK7: AND across terms; 3 + 3 outranks 3 + 2.
        assert_eq!(
            ranked("cash unemp"),
            vec!["File for unemployment", "Call unemployment office",]
        );
        // DK8: an empty query keeps canonical order.
        assert_eq!(
            ranked(""),
            vec![
                "File for unemployment",
                "Call unemployment office",
                "Dispute North Face jacket",
                "Launch swarm to find hospital",
                "Run e2e on sase-8v",
            ]
        );
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!(
                "{}-{}-{}-{}",
                prefix,
                std::process::id(),
                current_time_nanos(),
                TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap_or_else(|error| {
                panic!("create temp dir {}: {error}", path.display())
            });
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

    fn current_time_nanos() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_nanos()
    }
}

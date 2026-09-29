//! Active-task discovery for the `^` picker.
//!
//! A read-only scanner that lists In Progress (`/`) and Next (`*`)
//! tasks with block IDs from routable vault-root notes, annotated
//! with the open Pomodoro (if any) whose children hold the task's
//! dedicated `[[route#^id]]` Task Link. The editor-contract phase
//! wires this into `capture-complete`'s `active_task` context.

use std::{collections::HashMap, fs, io, path::Path};

use serde::Serialize;

use super::{
    capture, capture_pomodoros, capture_targets, capture_task_toggle,
    capture_tasks, note_tasks, pomodoro, projects,
};

/// An open Pomodoro holding a task's dedicated Task Link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ActiveTaskPomodoro {
    pub(crate) line: usize,
    pub(crate) name: Option<String>,
    pub(crate) time_range: Option<String>,
    pub(crate) is_current: bool,
}

/// One In Progress or Next task with a block ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ActiveTask {
    pub(crate) route: String,
    pub(crate) block_id: String,
    pub(crate) status_symbol: char,
    pub(crate) status_name: String,
    pub(crate) status_type: &'static str,
    pub(crate) text: String,
    pub(crate) section: Option<String>,
    #[serde(rename = "ref")]
    pub(crate) task_ref: String,
    pub(crate) line: usize,
    pub(crate) pomodoro: Option<ActiveTaskPomodoro>,
}

impl ActiveTask {
    /// `route:block-id`, the string a `^` accept inserts.
    pub(crate) fn replacement(&self) -> String {
        format!("{}:{}", self.route, self.block_id)
    }
}

/// Discovery output: ordered candidates plus bounded warnings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActiveTaskResult {
    pub(crate) tasks: Vec<ActiveTask>,
    pub(crate) warnings: Vec<String>,
}

/// Scan every routable vault-root note for active tasks.
///
/// With an empty query the order is: queued tasks in ledger order
/// (entry order, then child order), unqueued In Progress tasks,
/// then unqueued Next tasks. Within the unqueued groups tasks are
/// ordered by route, then line. A failed or partial read still
/// returns the other candidates alongside a warning.
pub(crate) fn discover(bob_dir: &Path) -> ActiveTaskResult {
    discover_at(bob_dir, &pomodoro::day_file_for(bob_dir))
}

/// [`discover`] with an explicit daily-note path, so tests can pass a
/// temp ledger without mutating the shared `BOB_DAY_FILE` override.
pub(crate) fn discover_at(bob_dir: &Path, day_file: &Path) -> ActiveTaskResult {
    let mut warnings = Vec::new();
    let routes = routable_routes(bob_dir, &mut warnings);
    let settings = note_tasks::read_settings(bob_dir);
    let mut tasks = Vec::new();
    for route in &routes {
        let contents =
            match fs::read_to_string(bob_dir.join(capture::route_label(route)))
            {
                Ok(contents) => contents,
                Err(error) => {
                    warnings.push(bounded_warning(format!(
                        "failed to read {}.md: {error}",
                        route
                    )));
                    continue;
                }
            };
        let scan = note_tasks::scan(&contents, &settings);
        tasks.extend(
            scan.open_tasks()
                .filter(|task| {
                    task.block_id.is_some()
                        && matches!(task.status_symbol, '/' | '*')
                })
                .map(|task| ActiveTask {
                    route: route.clone(),
                    block_id: task
                        .block_id
                        .clone()
                        .expect("filtered to tasks with a block ID"),
                    status_symbol: task.status_symbol,
                    status_name: task.status_name.clone(),
                    status_type: capture_tasks::status_type_label(
                        task.status_type,
                    ),
                    text: task.description.clone(),
                    section: task.section.clone(),
                    task_ref: task.task_ref(),
                    line: task.line_index + 1,
                    pomodoro: None,
                }),
        );
    }

    let ledger = read_ledger(day_file, &mut warnings);
    for task in &mut tasks {
        let key = (task.route.clone(), task.block_id.clone());
        task.pomodoro = ledger.owners.get(&key).cloned();
    }
    order_tasks(&mut tasks, &ledger.positions);
    ActiveTaskResult { tasks, warnings }
}

/// Rank candidates for a `^` query: prefix matches before
/// substring matches over `route:block-id`, the block ID, the task
/// text, the section, and the Pomodoro name. Order is stable
/// inside each tier, so `^dee`, `^sase:dee`, and `^outline` all
/// find their tasks. An empty query keeps every task.
pub(crate) fn rank<'a>(
    tasks: &'a [ActiveTask],
    query: &str,
) -> Vec<&'a ActiveTask> {
    if query.is_empty() {
        return tasks.iter().collect();
    }
    let query = query.to_lowercase();
    let mut prefix = Vec::new();
    let mut substring = Vec::new();
    for task in tasks {
        match match_kind(task, &query) {
            Some(MatchKind::Prefix) => prefix.push(task),
            Some(MatchKind::Substring) => substring.push(task),
            None => {}
        }
    }
    prefix.extend(substring);
    prefix
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchKind {
    Prefix,
    Substring,
}

fn match_kind(task: &ActiveTask, query: &str) -> Option<MatchKind> {
    let mut substring = false;
    for field in rank_fields(task) {
        let value = field.to_lowercase();
        if value.starts_with(query) {
            return Some(MatchKind::Prefix);
        }
        if value.contains(query) {
            substring = true;
        }
    }
    substring.then_some(MatchKind::Substring)
}

fn rank_fields(task: &ActiveTask) -> Vec<String> {
    let mut fields =
        vec![task.replacement(), task.block_id.clone(), task.text.clone()];
    if let Some(section) = &task.section {
        fields.push(section.clone());
    }
    if let Some(name) = task
        .pomodoro
        .as_ref()
        .and_then(|pomodoro| pomodoro.name.as_ref())
    {
        fields.push(name.clone());
    }
    fields
}

/// Vault-root `*.md` notes whose stem is a valid capture route,
/// reusing the `capture-targets` eligible-filename predicate. Area
/// and project frontmatter is deliberately ignored: every routable
/// note (including the inbox) can hold active tasks.
fn routable_routes(bob_dir: &Path, warnings: &mut Vec<String>) -> Vec<String> {
    let entries = match fs::read_dir(bob_dir) {
        Ok(entries) => entries,
        Err(error) => {
            warnings.push(bounded_warning(format!(
                "failed to read vault directory {}: {error}",
                bob_dir.display()
            )));
            return Vec::new();
        }
    };
    let mut entries = match entries.collect::<Result<Vec<_>, io::Error>>() {
        Ok(entries) => entries,
        Err(error) => {
            warnings.push(bounded_warning(format!(
                "failed to read vault directory {}: {error}",
                bob_dir.display()
            )));
            return Vec::new();
        }
    };
    entries.sort_by_key(|entry| entry.path());
    let mut routes = Vec::new();
    for entry in entries {
        let path = entry.path();
        let is_file = entry
            .file_type()
            .as_ref()
            .map(|file_type| file_type.is_file())
            .unwrap_or(false);
        if !is_file || !projects::is_markdown_file(&path) {
            continue;
        }
        if let Some(route) =
            capture_targets::routable_route_for_root_file(&path)
        {
            routes.push(route);
        }
    }
    routes
}

pub(crate) type TaskKey = (String, String);

#[derive(Debug, Default)]
pub(crate) struct Ledger {
    /// First open entry (in ledger order) holding each link.
    pub(crate) owners: HashMap<TaskKey, ActiveTaskPomodoro>,
    /// Ledger order per link: entry order, then child order.
    positions: HashMap<TaskKey, (usize, usize)>,
}

/// Today's open entries and their dedicated links.
pub(crate) fn read_ledger(
    day_file: &Path,
    warnings: &mut Vec<String>,
) -> Ledger {
    let contents = match fs::read_to_string(day_file) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            warnings.push(bounded_warning(format!(
                "Bob daily note does not exist: {}",
                day_file.display()
            )));
            return Ledger::default();
        }
        Err(error) => {
            warnings.push(bounded_warning(format!(
                "failed to read {}: {error}",
                day_file.display()
            )));
            return Ledger::default();
        }
    };
    if !capture_pomodoros::scan(&contents).has_section {
        warnings.push(bounded_warning(format!(
            "Bob daily note has no Pomodoros section: {}",
            day_file.display()
        )));
        return Ledger::default();
    }
    let mut ledger = Ledger::default();
    for (entry_index, entry) in
        capture_task_toggle::list_open_entry_links(&contents)
            .iter()
            .enumerate()
    {
        let pomodoro = ActiveTaskPomodoro {
            line: entry.line,
            name: entry.name.clone(),
            time_range: entry.time_range.clone(),
            is_current: entry.is_current,
        };
        for (link_index, link) in entry.links.iter().enumerate() {
            if let Some((route, id)) = parse_dedicated_link(link) {
                ledger
                    .owners
                    .entry((route.clone(), id.clone()))
                    .or_insert_with(|| pomodoro.clone());
                ledger
                    .positions
                    .entry((route, id))
                    .or_insert((entry_index, link_index));
            }
        }
    }
    ledger
}

/// Split a dedicated `[[route#^id]]` link into its route and block ID.
fn parse_dedicated_link(link: &str) -> Option<(String, String)> {
    let inner = link.strip_prefix("[[")?.strip_suffix("]]")?;
    let (route, fragment) = inner.split_once('#')?;
    let id = fragment.strip_prefix('^')?;
    if route.is_empty() || id.is_empty() {
        return None;
    }
    Some((route.to_string(), id.to_string()))
}

fn order_tasks(
    tasks: &mut Vec<ActiveTask>,
    positions: &HashMap<TaskKey, (usize, usize)>,
) {
    tasks.sort_by(|left, right| {
        queue_key(left, positions)
            .cmp(&queue_key(right, positions))
            .then_with(|| left.route.cmp(&right.route))
            .then_with(|| left.line.cmp(&right.line))
    });
}

fn queue_key(
    task: &ActiveTask,
    positions: &HashMap<TaskKey, (usize, usize)>,
) -> (u8, usize, usize) {
    let key = (task.route.clone(), task.block_id.clone());
    if let Some((entry, link)) = positions.get(&key) {
        return (0, *entry, *link);
    }
    if task.status_symbol == '/' {
        return (1, 0, 0);
    }
    (2, 0, 0)
}

fn bounded_warning(message: String) -> String {
    const LIMIT: usize = 300;
    if message.chars().count() <= LIMIT {
        return message;
    }
    let mut truncated = message.chars().take(LIMIT - 3).collect::<String>();
    truncated.push_str("...");
    truncated
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

    /// A temp ledger outside the vault root, so route enumeration never
    /// picks the daily note itself up as a task note.
    fn day_ledger(root: &Path, contents: &str) -> PathBuf {
        let day_file = root.join("ledger/day.md");
        write_file(&day_file, contents);
        day_file
    }

    fn discover_with_ledger(root: &Path, contents: &str) -> ActiveTaskResult {
        let day_file = day_ledger(root, contents);
        discover_at(root, &day_file)
    }

    fn write_standard_notes(root: &Path) {
        write_settings(root);
        write_file(
            &root.join("sase.md"),
            "# Tasks\n\
             - [/] Outline the fix #task ^outline\n\
             - [*] Fix deep bug #task ^deep-fix\n\
             - [ ] Ready thing #task ^ready\n\
             - [?] Blocked thing #task ^blocked\n\
             - [x] Done thing #task ^done\n\
             - [*] Parent without an ID #task\n\
             \x20 - [*] Nested next #task ^nested\n",
        );
        write_file(
            &root.join("work.md"),
            "# Tasks\n\
             - [/] Unqueued wip #task ^wip\n\
             - [*] Unqueued later #task ^later\n",
        );
        write_file(
            &root.join("mac_inbox.md"),
            "- [*] Inbox next #task ^inbox-next\n",
        );
        write_file(
            &root.join("Uppercase.md"),
            "- [*] Upper next #task ^upper-next\n",
        );
        write_file(
            &root.join("not a route.md"),
            "- [*] Spaced next #task ^spaced-next\n",
        );
    }

    fn replacements(result: &ActiveTaskResult) -> Vec<String> {
        result.tasks.iter().map(|task| task.replacement()).collect()
    }

    #[test]
    fn orders_queued_first_then_wip_then_next() {
        let temp = TempDir::new("bob-cli-active-tasks-order");
        write_standard_notes(temp.path());
        let result = discover_with_ledger(
            temp.path(),
            "## Pomodoros\n\
             - [ ] (**0905-0930** [t:: 25m]) — BUGS\n\
             \x20 - [[sase#^deep-fix]]\n\
             \x20 \x20 - repro notes\n\
             \x20 - [[mac_inbox#^inbox-next]]\n\
             - [ ] () — FOCUS\n\
             \x20 - [[work#^wip]]\n",
        );
        assert!(result.warnings.is_empty());
        assert_eq!(
            replacements(&result),
            vec![
                "sase:deep-fix",
                "mac_inbox:inbox-next",
                "work:wip",
                "sase:outline",
                "sase:nested",
                "work:later",
            ]
        );

        let deep_fix = &result.tasks[0];
        assert_eq!(deep_fix.status_symbol, '*');
        assert_eq!(deep_fix.status_name, "Next");
        assert_eq!(deep_fix.status_type, "ON_HOLD");
        assert_eq!(deep_fix.text, "Fix deep bug");
        assert_eq!(deep_fix.section.as_deref(), Some("Tasks"));
        let pomodoro = deep_fix.pomodoro.as_ref().expect("queued annotation");
        assert_eq!(pomodoro.name.as_deref(), Some("BUGS"));
        assert_eq!(pomodoro.line, 2);
        assert!(pomodoro.time_range.is_some());
        assert!(pomodoro.is_current);

        let wip = &result.tasks[2];
        assert_eq!(wip.status_symbol, '/');
        assert_eq!(wip.status_name, "In Progress");
        assert_eq!(wip.status_type, "IN_PROGRESS");
        let pomodoro = wip.pomodoro.as_ref().expect("queued annotation");
        assert_eq!(pomodoro.name.as_deref(), Some("FOCUS"));
        assert!(!pomodoro.is_current);

        assert!(result.tasks[3].pomodoro.is_none());
        assert!(result.tasks[4].pomodoro.is_none());
    }

    #[test]
    fn excludes_tasks_without_ids_and_closed_statuses() {
        let temp = TempDir::new("bob-cli-active-tasks-filter");
        write_standard_notes(temp.path());
        let result = discover_with_ledger(
            temp.path(),
            "## Pomodoros\n- [ ] () — BUGS\n",
        );
        let found: Vec<&str> = result
            .tasks
            .iter()
            .map(|task| task.block_id.as_str())
            .collect();
        for excluded in
            ["ready", "blocked", "done", "upper-next", "spaced-next"]
        {
            assert!(!found.contains(&excluded), "{excluded} must be excluded");
        }
        assert!(
            !result
                .tasks
                .iter()
                .any(|task| task.text.contains("without an ID")),
            "tasks without a block ID are excluded"
        );
        assert!(
            result.tasks.iter().any(|task| task.block_id == "nested"),
            "nested tasks are included"
        );
        assert!(
            result.tasks.iter().any(|task| task.route == "mac_inbox"),
            "mac_inbox.md is included"
        );
    }

    #[test]
    fn ranks_prefix_matches_before_substring_matches() {
        let temp = TempDir::new("bob-cli-active-tasks-rank");
        write_standard_notes(temp.path());
        let result = discover_with_ledger(
            temp.path(),
            "## Pomodoros\n\
             - [ ] (**0905-0930** [t:: 25m]) — BUGS\n\
             \x20 - [[sase#^deep-fix]]\n",
        );
        let ranked = rank(&result.tasks, "dee")
            .iter()
            .map(|task| task.replacement())
            .collect::<Vec<_>>();
        assert_eq!(ranked, vec!["sase:deep-fix"]);

        let ranked = rank(&result.tasks, "sase:dee")
            .iter()
            .map(|task| task.replacement())
            .collect::<Vec<_>>();
        assert_eq!(ranked, vec!["sase:deep-fix"]);

        let ranked = rank(&result.tasks, "outline")
            .iter()
            .map(|task| task.replacement())
            .collect::<Vec<_>>();
        assert_eq!(ranked, vec!["sase:outline"]);

        // "wip" is a prefix of the block ID and a substring of "Unqueued wip".
        let ranked = rank(&result.tasks, "wip")
            .iter()
            .map(|task| task.replacement())
            .collect::<Vec<_>>();
        assert_eq!(ranked.first().map(String::as_str), Some("work:wip"));

        // Pomodoro names are searchable: BUGS matches queued tasks.
        let ranked = rank(&result.tasks, "bugs")
            .iter()
            .map(|task| task.replacement())
            .collect::<Vec<_>>();
        assert!(ranked.contains(&"sase:deep-fix".to_string()));

        let all = rank(&result.tasks, "");
        assert_eq!(all.len(), result.tasks.len());
    }

    #[test]
    fn annotates_duplicate_links_with_the_first_owner() {
        let temp = TempDir::new("bob-cli-active-tasks-duplicate");
        write_settings(temp.path());
        write_file(
            &temp.path().join("sase.md"),
            "- [*] Duplicated #task ^dup\n",
        );
        let result = discover_with_ledger(
            temp.path(),
            "## Pomodoros\n\
             - [ ] () — AAA\n\
             \x20 - [[sase#^dup]]\n\
             - [ ] () — BBB\n\
             \x20 - [[sase#^dup]]\n",
        );
        assert_eq!(result.tasks.len(), 1);
        let pomodoro = result.tasks[0].pomodoro.as_ref().expect("queued");
        assert_eq!(pomodoro.name.as_deref(), Some("AAA"));
    }

    #[test]
    fn clears_is_current_with_multiple_open_timed_entries() {
        let temp = TempDir::new("bob-cli-active-tasks-timed");
        write_settings(temp.path());
        write_file(
            &temp.path().join("sase.md"),
            "- [*] Deep #task ^deep-fix\n",
        );
        let result = discover_with_ledger(
            temp.path(),
            "## Pomodoros\n\
             - [ ] (0900-0925) — AAA\n\
             \x20 - [[sase#^deep-fix]]\n\
             - [ ] (0930-0955) — BBB\n",
        );
        assert_eq!(result.tasks.len(), 1);
        let pomodoro = result.tasks[0].pomodoro.as_ref().expect("queued");
        assert_eq!(pomodoro.name.as_deref(), Some("AAA"));
        assert!(!pomodoro.is_current);
    }

    #[test]
    fn warns_when_the_day_file_is_missing() {
        let temp = TempDir::new("bob-cli-active-tasks-no-day");
        write_settings(temp.path());
        write_file(
            &temp.path().join("sase.md"),
            "- [*] Deep #task ^deep-fix\n",
        );
        let missing = temp.path().join("2010/20100101.md");
        let result = discover_at(temp.path(), &missing);
        assert_eq!(result.tasks.len(), 1);
        assert!(result.tasks[0].pomodoro.is_none());
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("does not exist"));
    }

    #[test]
    fn warns_when_the_pomodoros_section_is_missing() {
        let temp = TempDir::new("bob-cli-active-tasks-no-section");
        write_settings(temp.path());
        write_file(
            &temp.path().join("sase.md"),
            "- [*] Deep #task ^deep-fix\n",
        );
        let result = discover_with_ledger(
            temp.path(),
            "# Just a note\n- [*] Deep #task ^deep-fix\n",
        );
        assert_eq!(result.tasks.len(), 1);
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("no Pomodoros section"));
    }

    #[test]
    fn warns_for_unreadable_notes_and_keeps_other_candidates() {
        let temp = TempDir::new("bob-cli-active-tasks-unreadable");
        write_settings(temp.path());
        write_file(
            &temp.path().join("sase.md"),
            "- [*] Deep #task ^deep-fix\n",
        );
        let locked = temp.path().join("locked.md");
        write_file(&locked, "- [*] Locked #task ^locked\n");
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
            result.tasks.iter().any(|task| task.block_id == "deep-fix"),
            "a failed read still returns the other candidates"
        );
        if result.tasks.iter().any(|task| task.block_id == "locked") {
            // Running with privileges that ignore file permissions; the
            // candidate set is complete so no warning is required.
            return;
        }
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("failed to read locked.md"));
        assert!(!result.warnings[0].contains("Locked"));
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

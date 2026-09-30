//! Ledger-derived Today engine: the open tasks with a dedicated
//! Task Link under today's open Pomodoros. `docs/plan.md` is the
//! authoritative definition; this module is its Rust implementation,
//! shared by `bob plan` and the task-status hooks. The
//! `bob-ledger-tools` JavaScript mirror shares the conformance
//! vectors in `docs/plan.md`.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use serde::Serialize;

use super::{super::vault_links::VaultLinkResolver, PlanLint, UNNAMED_THEME};
use crate::native::{capture_pomodoro_start, capture_pomodoros, note_tasks};

/// Lint code for a ledger link that resolves to no countable task:
/// a missing or ambiguous note, an unreadable file, or a block ID
/// with no open task behind it.
pub(crate) const LINT_TODAY_UNRESOLVED: &str = "today_link_unresolved";

/// One dedicated Task Link under an open Pomodoro entry: the entry's
/// 1-based line and name plus the link's 1-based ledger line, written
/// target, block ID, and embed flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TodayLink {
    pub(crate) entry_line: usize,
    pub(crate) entry_name: String,
    pub(crate) ledger_line: usize,
    pub(crate) target: String,
    pub(crate) block_id: String,
    pub(crate) embedded: bool,
}

/// One resolved Today task, in ledger order of first occurrence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TodayTask {
    pub(crate) path: String,
    pub(crate) block_id: String,
    pub(crate) line: usize,
    pub(crate) status_symbol: String,
    pub(crate) status_name: String,
    pub(crate) text: String,
    pub(crate) entry_line: usize,
    pub(crate) entry_name: String,
    pub(crate) ledger_line: usize,
}

/// The resolved Today rows plus non-blocking lints for links that
/// point at nothing countable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TodayResult {
    pub(crate) tasks: Vec<TodayTask>,
    pub(crate) warnings: Vec<PlanLint>,
}

/// Every dedicated Task Link under today's open entries, in ledger
/// order. This is exactly the `=x` / start lineup rule
/// ([`capture_pomodoro_start::list_queued_links`]): a direct child
/// bullet at the entry's first child indentation whose body, after
/// stripping Pomodoro markers, is exactly one plain or embedded
/// block link. Exempt entries, unnamed placeholders, mixed-text
/// bullets, deeper descendants, struck links, and fenced lines
/// behave exactly as the lineup does.
pub(crate) fn today_links(contents: &str) -> Vec<TodayLink> {
    let scan = capture_pomodoros::scan(contents);
    let mut links = Vec::new();
    for entry in &scan.entries {
        if entry.state != capture_pomodoros::PomodoroState::Open {
            continue;
        }
        let Some(entry_index) = entry.line.checked_sub(1) else {
            continue;
        };
        let entry_name = entry
            .name
            .clone()
            .unwrap_or_else(|| UNNAMED_THEME.to_string());
        for queued in
            capture_pomodoro_start::list_queued_links(contents, entry_index)
        {
            links.push(TodayLink {
                entry_line: entry.line,
                entry_name: entry_name.clone(),
                ledger_line: queued.ledger_line,
                target: queued.path_part,
                block_id: queued.block_id,
                embedded: queued.embedded,
            });
        }
    }
    links
}

/// Resolve [`today_links`] against the vault: an empty target is the
/// daily note itself, otherwise the hooks' rules apply (exact
/// vault-relative path with or without `.md`, then a unique
/// case-insensitive basename). Unresolved or ambiguous links are
/// skipped with a [`LINT_TODAY_UNRESOLVED`] lint; resolved `#task`
/// lines with that block ID count only when their status is open
/// (done and cancelled tasks drop out silently); rows deduplicate by
/// `(path, block ID)`, keeping the ledger order of first occurrence.
/// Transcluded dependencies never inherit Today: only links the
/// ledger lists directly count.
///
/// `daily_relative` is the daily note's vault-relative path with
/// `.md` (for example `2026/20261001.md`).
pub(crate) fn today_tasks(
    bob_dir: &Path,
    daily_relative: &str,
    contents: &str,
) -> TodayResult {
    let day_path = bob_dir.join(daily_relative);
    let resolver = VaultLinkResolver::new(bob_dir);
    let settings = note_tasks::read_settings(bob_dir);
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    let mut tasks = Vec::new();
    let mut warnings = Vec::new();
    for link in today_links(contents) {
        let target = link.target.trim();
        let relative = if target.is_empty() {
            PathBuf::from(daily_relative)
        } else {
            match resolver.resolve(&day_path, target).path() {
                Some(path) => path.to_path_buf(),
                None => {
                    warnings.push(unresolved(
                        &link,
                        "does not resolve to a vault note",
                    ));
                    continue;
                }
            }
        };
        let display = relative.to_string_lossy().replace('\\', "/");
        let absolute = bob_dir.join(&relative);
        let file_contents = match fs::read_to_string(&absolute) {
            Ok(contents) => contents,
            Err(_) => {
                warnings.push(unresolved(
                    &link,
                    &format!("resolves to {display}, which cannot be read"),
                ));
                continue;
            }
        };
        let scan = note_tasks::scan(&file_contents, &settings);
        let task = match scan.by_block_id(&link.block_id) {
            note_tasks::BlockIdLookup::Found(task) => task,
            note_tasks::BlockIdLookup::Duplicate(count) => {
                warnings.push(unresolved(
                    &link,
                    &format!("{display} has {count} lines with this block id"),
                ));
                continue;
            }
            note_tasks::BlockIdLookup::NotATask { .. }
            | note_tasks::BlockIdLookup::Missing => {
                warnings.push(unresolved(
                    &link,
                    &format!("{display} has no task with this block id"),
                ));
                continue;
            }
        };
        if !task.status_type.is_open() {
            continue;
        }
        if !seen.insert((display.clone(), link.block_id.clone())) {
            continue;
        }
        tasks.push(TodayTask {
            path: display,
            block_id: link.block_id.clone(),
            line: task.line_index + 1,
            status_symbol: task.status_symbol.to_string(),
            status_name: task.status_name.clone(),
            text: task.description.clone(),
            entry_line: link.entry_line,
            entry_name: link.entry_name.clone(),
            ledger_line: link.ledger_line,
        });
    }
    TodayResult { tasks, warnings }
}

fn unresolved(link: &TodayLink, reason: &str) -> PlanLint {
    PlanLint {
        code: LINT_TODAY_UNRESOLVED.to_string(),
        message: format!("[[{}#^{}]] {reason}", link.target, link.block_id,),
        line: Some(link.ledger_line),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TempVault {
        path: PathBuf,
    }

    impl TempVault {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "bob-today-{}-{}",
                std::process::id(),
                TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).expect("create temp vault");
            let vault = Self { path };
            vault.write(
                ".obsidian/plugins/obsidian-tasks-plugin/data.json",
                "{\"globalFilter\":\"#task\",\"taskFormat\":\"dataview\",\
                \"statusSettings\":{\"coreStatuses\":[\
                {\"symbol\":\" \",\"name\":\"Ready\",\"nextStatusSymbol\":\"x\",\
                \"availableAsCommand\":true,\"type\":\"TODO\"},\
                {\"symbol\":\"x\",\"name\":\"Done\",\"nextStatusSymbol\":\" \",\
                \"availableAsCommand\":true,\"type\":\"DONE\"}],\
                \"customStatuses\":[\
                {\"symbol\":\"/\",\"name\":\"In Progress\",\
                \"nextStatusSymbol\":\"x\",\"availableAsCommand\":true,\
                \"type\":\"IN_PROGRESS\"},\
                {\"symbol\":\"*\",\"name\":\"Next\",\"nextStatusSymbol\":\"x\",\
                \"availableAsCommand\":true,\"type\":\"ON_HOLD\"},\
                {\"symbol\":\"?\",\"name\":\"Blocked\",\"nextStatusSymbol\":\" \",\
                \"availableAsCommand\":true,\"type\":\"ON_HOLD\"},\
                {\"symbol\":\"-\",\"name\":\"Canceled\",\"nextStatusSymbol\":\" \",\
                \"availableAsCommand\":true,\"type\":\"CANCELLED\"}]}}\n",
            );
            vault
        }

        fn write(&self, relative: &str, contents: &str) {
            let path = self.path.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(&path, contents).expect("write vault file");
        }

        fn result(&self, daily: &str, contents: &str) -> TodayResult {
            today_tasks(&self.path, daily, contents)
        }
    }

    fn keys(result: &TodayResult) -> Vec<String> {
        result
            .tasks
            .iter()
            .map(|task| format!("{}#{}", task.path, task.block_id))
            .collect()
    }

    impl Drop for TempVault {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    /// T1 GTD: an exempt entry's empty-target link resolves to the
    /// daily note itself.
    #[test]
    fn gtd_empty_target_resolves_to_the_daily_note() {
        let vault = TempVault::new();
        let daily = "2026/20261001.md";
        let contents = "# 2026-10-01\n\
            \n\
            ## Pomodoros\n\
            \n\
            - [ ] () — GTD\n\
            \t- [[#^gtd]]\n\
            \n\
            ## Tasks\n\
            \n\
            - [ ] #task Gtd chore ^gtd\n";
        vault.write(daily, contents);
        let result = vault.result(daily, contents);
        assert_eq!(keys(&result), vec!["2026/20261001.md#gtd"]);
        assert!(result.warnings.is_empty());
        assert_eq!(result.tasks[0].entry_name, "GTD");
        assert_eq!(result.tasks[0].status_symbol, " ");
    }

    /// T2 markers: Pomodoro markers count, struck links do not, and
    /// embedded links count.
    #[test]
    fn markers_struck_and_embedded_follow_the_lineup_rule() {
        let vault = TempVault::new();
        vault.write("a.md", "- [ ] #task Ex ^x\n- [ ] #task Zed ^z\n");
        let contents = "## Pomodoros\n\
            \n\
            - [ ] () — GOALS\n\
            \x20   - 🍅 [[a#^x]]\n\
            \x20   - ~~[[a#^y]]~~\n\
            \x20   - ![[a#^z]]\n";
        let links = today_links(contents);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].block_id, "x");
        assert!(!links[0].embedded);
        assert_eq!(links[1].block_id, "z");
        assert!(links[1].embedded);
        let result = vault.result("2026/20261001.md", contents);
        assert_eq!(keys(&result), vec!["a.md#x", "a.md#z"]);
    }

    /// T3 closed entries: links under completed or cancelled
    /// Pomodoros never count.
    #[test]
    fn closed_entries_contribute_no_links() {
        let contents = "## Pomodoros\n\
            \n\
            - [x] () — DONE\n\
            \x20   - [[a#^x]]\n\
            - [-] () — GONE\n\
            \x20   - [[a#^y]]\n\
            - [ ] () — OPEN\n";
        assert!(today_links(contents).is_empty());
    }

    /// T4 shapes: a mixed-text bullet and a link nested under a
    /// note bullet are not dedicated Task Links.
    #[test]
    fn mixed_and_nested_bullets_are_not_dedicated_links() {
        let contents = "## Pomodoros\n\
            \n\
            - [ ] () — GOALS\n\
            \x20   - Review [[a#^m]]\n\
            \x20   - Note text\n\
            \x20       - [[a#^deep]]\n";
        assert!(today_links(contents).is_empty());
    }

    /// T5 dedupe: one task under two open entries, plus an exact
    /// path and a basename resolving to the same note, yields one
    /// key at its first position.
    #[test]
    fn duplicate_links_dedupe_to_the_first_position() {
        let vault = TempVault::new();
        vault.write("dir/a.md", "- [*] #task Ex ^x\n");
        let contents = "## Pomodoros\n\
            \n\
            - [ ] () — ONE\n\
            \x20   - [[a#^x]]\n\
            - [ ] () — TWO\n\
            \x20   - [[a#^x]]\n\
            \x20   - [[dir/a#^x]]\n";
        let result = vault.result("2026/20261001.md", contents);
        assert_eq!(keys(&result), vec!["dir/a.md#x"]);
        assert_eq!(result.tasks[0].entry_line, 3);
        assert_eq!(result.tasks[0].ledger_line, 4);
        assert_eq!(result.tasks[0].status_symbol, "*");
        assert_eq!(result.tasks[0].status_name, "Next");
    }

    /// T6 fenced: a link inside a fenced block never counts.
    #[test]
    fn fenced_links_never_count() {
        let vault = TempVault::new();
        vault.write("a.md", "- [ ] #task Ex ^x\n- [ ] #task Why ^q\n");
        let contents = "## Pomodoros\n\
            \n\
            - [ ] () — GOALS\n\
            \x20   - [[a#^x]]\n\
            \n\
            ```\n\
            - [[a#^q]]\n\
            ```\n";
        let result = vault.result("2026/20261001.md", contents);
        assert_eq!(keys(&result), vec!["a.md#x"]);
    }

    /// T7 status: done and cancelled tasks drop out silently while
    /// Blocked stays.
    #[test]
    fn done_and_cancelled_tasks_drop_out_while_blocked_stays() {
        let vault = TempVault::new();
        vault.write(
            "a.md",
            "- [x] #task Done ^x\n\
            - [-] #task Gone ^y\n\
            - [?] #task Waiting ^z\n\
            - [ ] #task Open ^w\n",
        );
        let contents = "## Pomodoros\n\
            \n\
            - [ ] () — GOALS\n\
            \x20   - [[a#^x]]\n\
            \x20   - [[a#^y]]\n\
            \x20   - [[a#^z]]\n\
            \x20   - [[a#^w]]\n";
        let result = vault.result("2026/20261001.md", contents);
        assert_eq!(keys(&result), vec!["a.md#z", "a.md#w"]);
        assert!(result.warnings.is_empty());
        assert_eq!(result.tasks[0].status_symbol, "?");
        assert_eq!(result.tasks[0].status_name, "Blocked");
    }

    /// T8 unresolved: a missing note yields no key plus
    /// `today_link_unresolved` at the ledger line.
    #[test]
    fn missing_notes_lint_and_drop_out() {
        let vault = TempVault::new();
        let contents = "## Pomodoros\n\
            \n\
            - [ ] () — GOALS\n\
            \x20   - [[missing#^q]]\n";
        let result = vault.result("2026/20261001.md", contents);
        assert!(result.tasks.is_empty());
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(result.warnings[0].code, LINT_TODAY_UNRESOLVED);
        assert_eq!(result.warnings[0].line, Some(4));
    }

    /// T9 alias: an aliased link pins whatever the lineup rule
    /// does with aliases.
    #[test]
    fn aliased_links_pin_the_lineup_behavior() {
        let vault = TempVault::new();
        vault.write("a.md", "- [ ] #task Ex ^x\n");
        let contents = "## Pomodoros\n\
            \n\
            - [ ] () — GOALS\n\
            \x20   - [[a#^x|alias]]\n";
        let links = today_links(contents);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].block_id, "x");
        let result = vault.result("2026/20261001.md", contents);
        assert_eq!(keys(&result), vec!["a.md#x"]);
    }
}

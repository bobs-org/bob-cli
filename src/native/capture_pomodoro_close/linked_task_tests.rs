use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use chrono::{NaiveDate, NaiveDateTime};
use tempfile::TempDir;

use super::super::vault_links::LinkResolution;
use super::linked_tasks::*;

struct MemoryVault {
    root: PathBuf,
    _directory: TempDir,
    files: BTreeMap<PathBuf, String>,
}

impl MemoryVault {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary vault");
        Self {
            root: directory.path().to_path_buf(),
            _directory: directory,
            files: BTreeMap::new(),
        }
    }

    fn insert(&mut self, path: &str, contents: &str) {
        self.files.insert(PathBuf::from(path), contents.to_string());
    }
}

impl CloseVault for MemoryVault {
    fn bob_dir(&self) -> &Path {
        &self.root
    }

    fn resolve_target(&self, from_path: &Path, target: &str) -> LinkResolution {
        if target.is_empty() {
            return LinkResolution::Found(from_path.to_path_buf());
        }
        let Some(candidate) =
            super::super::vault_links::target_to_markdown_path(target)
        else {
            return LinkResolution::Missing;
        };
        if self.files.contains_key(&candidate) {
            return LinkResolution::Found(candidate);
        }
        if target.contains('/') || target.contains('\\') {
            return LinkResolution::Missing;
        }
        let stem = target.strip_suffix(".md").unwrap_or(target);
        let mut matches = self.files.keys().filter(|path| {
            path.file_stem()
                .and_then(|part| part.to_str())
                .is_some_and(|part| part.eq_ignore_ascii_case(stem))
        });
        match (matches.next(), matches.next()) {
            (Some(path), None) => LinkResolution::Found(path.clone()),
            (Some(_), Some(_)) => LinkResolution::Ambiguous,
            _ => LinkResolution::Missing,
        }
    }

    fn read_latest(&self, path: &Path) -> Result<Option<String>, String> {
        Ok(self.files.get(path).cloned())
    }
}

fn at(hour: u32, minute: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 9, 28)
        .expect("date")
        .and_hms_opt(hour, minute, 0)
        .expect("time")
}

fn run_close(
    vault: &MemoryVault,
    day_path: &Path,
    day_contents: &str,
) -> PomodoroClosePlan {
    plan_pomodoro_close(day_path, day_contents, at(9, 37), vault, None)
        .expect("close plan")
}

fn worked_example_day() -> String {
    concat!(
        "## Pomodoros\n\n",
        "- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
        "  - 🍅 [[bob#^capture-stop]]\n",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE\n",
        "  - [[bob#^capture-stop]]\n",
        "    - Designed the `=x` grammar\n",
        "      - chose `x` for done\n",
        "    - Wrote the plan\n",
        "  - [[bob#^web-capture]]#\n",
        "  - ~~[[sase#^axe-restart]]~~\n",
        "    - Restarted axe\n",
        "  - quick note\n",
        "- [ ] () — SASE\n",
        "  - [[sase#^recovery-panel]]\n",
    )
    .to_string()
}

#[test]
fn worked_example_updates_tasks_and_writes_dated_work_logs() {
    let mut vault = MemoryVault::new();
    vault.insert(
        "bob.md",
        concat!(
            "- [*] #task Add support for `=x` syntax! [created::2026-09-26] ^capture-stop\n",
            "- [*] #task Add capture support for web URLs! [created::2026-09-21] ^web-capture\n",
            "- [ ] #task Plain ready task [created::2026-09-20] ^ready\n",
        ),
    );
    vault.insert(
        "sase.md",
        concat!(
            "- [x] #task Restart axe [created::2026-09-27] [completion:: 2026-09-28] ^axe-restart\n",
            "\t- 🛠️ **WORK LOG**\n",
            "\t\t- _2026-09-27_ — Diagnosed the hang\n",
            "- [ ] #task Recovery panel [created::2026-09-25] ^recovery-panel\n",
        ),
    );
    let day_path = Path::new("2026/20260928.md");
    let plan = run_close(&vault, day_path, &worked_example_day());

    assert_eq!(
        plan.changed_files.get(Path::new("bob.md")).map(String::as_str),
        Some(concat!(
            "- [/] #task Add support for `=x` syntax! [created::2026-09-26] ^capture-stop\n",
            "\t- 🛠️ **WORK LOG**\n",
            "\t\t- *2026-09-28* — Designed the `=x` grammar\n",
            "\t\t\t- chose `x` for done\n",
            "\t\t- *2026-09-28* — Wrote the plan\n",
            "- [*] #task Add capture support for web URLs! [created::2026-09-21] ^web-capture\n",
            "- [ ] #task Plain ready task [created::2026-09-20] ^ready\n",
        ))
    );
    assert_eq!(
        plan.changed_files.get(Path::new("sase.md")).map(String::as_str),
        Some(concat!(
            "- [x] #task Restart axe [created::2026-09-27] [completion:: 2026-09-28] ^axe-restart\n",
            "\t- 🛠️ **WORK LOG**\n",
            "\t\t- *2026-09-28* — Restarted axe\n",
            "\t\t- _2026-09-27_ — Diagnosed the hang\n",
            "- [ ] #task Recovery panel [created::2026-09-25] ^recovery-panel\n",
        ))
    );
    assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);
    assert_eq!(plan.summary.tasks.len(), 3);
    assert_eq!(plan.summary.tasks[0].role, CloseTaskRole::Worked);
    assert_eq!(plan.summary.tasks[0].status_symbol, Some('/'));
    assert_eq!(plan.summary.tasks[0].work_log.len(), 2);
    assert_eq!(plan.summary.tasks[1].role, CloseTaskRole::Deferred);
    assert_eq!(plan.summary.tasks[1].status_symbol, Some('*'));
    assert_eq!(plan.summary.tasks[2].role, CloseTaskRole::Struck);
    assert_eq!(plan.summary.tasks[2].work_log.len(), 1);
}

#[test]
fn recursively_closes_embedded_tasks_and_retires_closed_ledger_embeds() {
    let mut vault = MemoryVault::new();
    vault.insert(
        "a.md",
        "- [ ] #task Parent [completion:: 2026-01-01] ^a\n\t- ![[b#^b]]\n",
    );
    vault.insert("b.md", "- [/] Child without task tag ^b\n");
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE\n",
        "\t- ![[a#^a]]\n",
    );
    let path = Path::new("2026/20260928.md");
    let plan = run_close(&vault, path, day);

    assert!(plan.changed_files[Path::new("a.md")]
        .contains("[completion:: 2026-09-28] ^a"));
    assert!(!plan.changed_files[Path::new("a.md")]
        .contains("[completion:: 2026-01-01]"));
    assert_eq!(
        plan.changed_files
            .get(Path::new("b.md"))
            .map(String::as_str),
        Some("- [x] Child without task tag ^b\n")
    );
    assert!(plan.summary.ledger.contents.contains("\t- ~~[[a#^a]]~~"));
    assert_eq!(plan.summary.tasks[0].role, CloseTaskRole::Embedded);
    assert_eq!(plan.summary.tasks[0].status_symbol, Some('x'));
    assert!(plan.summary.tasks.iter().any(|task| task.role
        == CloseTaskRole::Subtask
        && task.block_id == "b"));
}

#[test]
fn embedded_recursion_obeys_depth_and_target_caps() {
    let mut vault = MemoryVault::new();
    for index in 0..27 {
        let next = index + 1;
        let children = if next < 27 {
            format!("\n\t- ![[T{next}#^t{next}]]\n")
        } else {
            "\n".to_string()
        };
        vault.insert(
            &format!("T{index}.md"),
            &format!("- [ ] #task T{index} ^t{index}{children}"),
        );
    }
    let chain_day = concat!(
        "## Pomodoros\n",
        "- [ ] (**0920-0950** [t:: 30m])\n",
        "\t- ![[T0#^t0]]\n",
    );
    let chain = run_close(&vault, Path::new("2026/20260928.md"), chain_day);
    assert_eq!(
        chain.changed_files[Path::new("T25.md")].lines().next(),
        Some("- [x] #task T25  [completion:: 2026-09-28] ^t25")
    );
    assert!(!chain.changed_files.contains_key(Path::new("T26.md")));

    let mut wide = MemoryVault::new();
    let mut child_links = String::new();
    for index in 0..251 {
        wide.insert(
            &format!("W{index}.md"),
            &format!("- [ ] #task W{index} ^w{index}\n"),
        );
        child_links.push_str(&format!("\t- ![[W{index}#^w{index}]]\n"));
    }
    wide.insert("Root.md", &format!("- [ ] #task Root ^root\n{child_links}"));
    let wide_day =
        "## Pomodoros\n- [ ] (**0920-0950** [t:: 30m])\n\t- ![[Root#^root]]\n";
    let wide_plan = run_close(&wide, Path::new("2026/20260928.md"), wide_day);
    assert!(wide_plan.changed_files[Path::new("W248.md")].starts_with("- [x]"));
    assert!(!wide_plan.changed_files.contains_key(Path::new("W249.md")));
    assert!(wide_plan
        .warnings
        .iter()
        .any(|warning| warning.contains("250 targets")));
}

#[test]
fn blocked_done_and_in_progress_bare_targets_are_not_started() {
    let mut vault = MemoryVault::new();
    vault.insert(
        "tasks.md",
        concat!(
            "- [?] #task Blocked ^blocked\n",
            "- [x] #task Done ^done\n",
            "- [/] #task In progress ^progress\n",
        ),
    );
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] (**0920-0950** [t:: 30m])\n",
        "\t- [[tasks#^blocked]]\n",
        "\t- [[tasks#^done]]\n",
        "\t- [[tasks#^progress]]\n",
    );
    let plan = run_close(&vault, Path::new("2026/20260928.md"), day);
    assert_eq!(plan.changed_files.get(Path::new("tasks.md")), None);
}

#[test]
fn unresolved_ambiguous_duplicate_and_non_task_links_warn_and_skip() {
    let mut vault = MemoryVault::new();
    vault.insert("one/dupe.md", "- [ ] #task One ^one\n");
    vault.insert("two/dupe.md", "- [ ] #task Two ^two\n");
    vault.insert(
        "duplicates.md",
        "- [ ] #task First ^dup\n- [ ] #task Second ^dup\n",
    );
    vault.insert("not-task.md", "- Plain item ^plain\n");
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] (**0920-0950** [t:: 30m])\n",
        "\t- [[missing#^missing]]\n",
        "\t- [[dupe#^one]]\n",
        "\t- [[duplicates#^dup]]\n",
        "\t- [[not-task#^plain]]\n",
    );
    let plan = run_close(&vault, Path::new("2026/20260928.md"), day);
    assert!(plan
        .warnings
        .iter()
        .any(|message| message.contains("does not resolve")));
    assert!(plan
        .warnings
        .iter()
        .any(|message| message.contains("ambiguous")));
    assert!(plan
        .warnings
        .iter()
        .any(|message| message.contains("duplicate ^dup")));
    assert!(plan
        .warnings
        .iter()
        .any(|message| message.contains("non-task")));
    assert!(!plan.changed_files.contains_key(Path::new("duplicates.md")));
    assert!(!plan.changed_files.contains_key(Path::new("not-task.md")));
}

#[test]
fn day_file_can_also_be_a_task_note_and_receives_its_work_log() {
    let vault = MemoryVault::new();
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] (**0920-0950** [t:: 30m])\n",
        "\t- [[#^self]]\n",
        "\t\t- Drafted this task's notes\n",
        "## Tasks\n",
        "- [ ] #task Same daily note [created:: 2026-09-20] ^self\n",
    );
    let path = Path::new("2026/20260928.md");
    let plan = run_close(&vault, path, day);
    let post = &plan.changed_files[path];
    assert!(post
        .contains("- [/] #task Same daily note [created:: 2026-09-20] ^self"));
    assert!(post.contains("*2026-09-28* — Drafted this task's notes"));
    assert_eq!(plan.changed_files.len(), 1);
}

#[test]
fn close_plan_preserves_crlf_in_changed_task_notes() {
    let mut vault = MemoryVault::new();
    vault.insert("bob.md", "- [ ] #task Ready ^ready\r\n");
    let day = "## Pomodoros\r\n- [ ] (**0920-0950** [t:: 30m])\r\n\t- [[bob#^ready]]\r\n";
    let plan = run_close(&vault, Path::new("2026/20260928.md"), day);
    let bob = &plan.changed_files[Path::new("bob.md")];
    assert_eq!(bob, "- [/] #task Ready ^ready\r\n");
    assert!(!bob.replace("\r\n", "").contains('\n'));
    assert!(!plan
        .summary
        .ledger
        .contents
        .replace("\r\n", "")
        .contains('\n'));
}

fn close_with_selection(
    vault: &MemoryVault,
    day_path: &Path,
    day_contents: &str,
    selection: &super::selection::CloseSelection,
) -> PomodoroClosePlan {
    plan_pomodoro_close(
        day_path,
        day_contents,
        at(9, 37),
        vault,
        Some(selection),
    )
    .expect("close plan")
}

fn selection(
    in_progress: Option<Vec<u32>>,
    complete: Vec<u32>,
    raw: &str,
) -> super::selection::CloseSelection {
    use std::collections::BTreeSet;
    super::selection::CloseSelection {
        in_progress: in_progress
            .map(|list| list.into_iter().collect::<BTreeSet<u32>>()),
        complete: complete.into_iter().collect::<BTreeSet<u32>>(),
        raw: raw.to_string(),
    }
}

fn worked_vault() -> MemoryVault {
    let mut vault = MemoryVault::new();
    vault.insert(
        "bob.md",
        concat!(
            "- [*] #task Add support for `=x` syntax! [created::2026-09-26] ^capture-stop\n",
            "- [*] #task Add capture support for web URLs! [created::2026-09-21] ^web-capture\n",
            "- [ ] #task Plain ready task [created::2026-09-20] ^ready\n",
        ),
    );
    vault.insert(
        "sase.md",
        concat!(
            "- [x] #task Restart axe [created::2026-09-27] [completion:: 2026-09-28] ^axe-restart\n",
            "\t- \u{1F6E0}\u{FE0F} **WORK LOG**\n",
            "\t\t- _2026-09-27_ \u{2014} Diagnosed the hang\n",
            "- [ ] #task Recovery panel [created::2026-09-25] ^recovery-panel\n",
        ),
    );
    vault
}

#[test]
fn selection_complete_writes_done_task_with_completion_date() {
    let vault = worked_vault();
    let day_path = Path::new("2026/20260928.md");
    let plan = close_with_selection(
        &vault,
        day_path,
        &worked_example_day(),
        &selection(None, vec![1], "=x!1"),
    );
    assert_eq!(
        plan.changed_files.get(Path::new("bob.md")).map(String::as_str),
        Some(concat!(
            "- [x] #task Add support for `=x` syntax! [created::2026-09-26]  [completion:: 2026-09-28] ^capture-stop\n",
            "\t- \u{1F6E0}\u{FE0F} **WORK LOG**\n",
            "\t\t- *2026-09-28* \u{2014} Designed the `=x` grammar\n",
            "\t\t\t- chose `x` for done\n",
            "\t\t- *2026-09-28* \u{2014} Wrote the plan\n",
            "- [*] #task Add capture support for web URLs! [created::2026-09-21] ^web-capture\n",
            "- [ ] #task Plain ready task [created::2026-09-20] ^ready\n",
        ))
    );
    assert_eq!(plan.summary.tasks[0].index, Some(1));
    assert_eq!(plan.summary.tasks[1].index, Some(2));
}

#[test]
fn selection_in_progress_and_complete_updates_both_tasks() {
    let vault = worked_vault();
    let day_path = Path::new("2026/20260928.md");
    let plan = close_with_selection(
        &vault,
        day_path,
        &worked_example_day(),
        &selection(Some(vec![1]), vec![2], "=x1!2"),
    );
    assert_eq!(
        plan.changed_files.get(Path::new("bob.md")).map(String::as_str),
        Some(concat!(
            "- [/] #task Add support for `=x` syntax! [created::2026-09-26] ^capture-stop\n",
            "\t- \u{1F6E0}\u{FE0F} **WORK LOG**\n",
            "\t\t- *2026-09-28* \u{2014} Designed the `=x` grammar\n",
            "\t\t\t- chose `x` for done\n",
            "\t\t- *2026-09-28* \u{2014} Wrote the plan\n",
            "- [x] #task Add capture support for web URLs! [created::2026-09-21]  [completion:: 2026-09-28] ^web-capture\n",
            "- [ ] #task Plain ready task [created::2026-09-20] ^ready\n",
        ))
    );
}

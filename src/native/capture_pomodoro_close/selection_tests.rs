use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use chrono::{NaiveDate, NaiveDateTime};
use tempfile::TempDir;

use super::super::vault_links::LinkResolution;
use super::linked_tasks::CloseVault;
use super::selection::{
    apply_close_selection, number_task_links, CloseSelection,
    CloseSelectionError, TaskLinkMarker, TaskLinkOutcome, TaskLinkSource,
};
use super::{find_running_pomodoro, plan_ledger_close, plan_pomodoro_close};

fn at(hour: u32, minute: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 9, 28)
        .expect("valid date")
        .and_hms_opt(hour, minute, 0)
        .expect("valid time")
}

fn note(lines: &[&str]) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

fn worked_example() -> String {
    note(&[
        "## Pomodoros",
        "",
        "- [x] (**0830-0855** [t:: 25m]) — PLAN",
        "\t- 🍅 [[bob#^capture-stop]]",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "\t- [[bob#^capture-stop]]",
        "\t\t- Designed the `=x` grammar",
        "\t\t\t- chose `x` for done",
        "\t\t- Wrote the plan",
        "\t- [[bob#^web-capture]]#",
        "\t- ~~[[sase#^axe-restart]]~~",
        "\t\t- Restarted axe",
        "\t- quick note",
        "- [ ] () — SASE",
        "\t- [[sase#^recovery-panel]]",
    ])
}

fn running_of(contents: &str) -> super::RunningPomodoro {
    find_running_pomodoro(contents).expect("running pomodoro")
}

fn sel(
    in_progress: Option<Vec<u32>>,
    complete: Vec<u32>,
    raw: &str,
) -> CloseSelection {
    sel_drop(in_progress, complete, Vec::new(), raw)
}

fn sel_drop(
    in_progress: Option<Vec<u32>>,
    complete: Vec<u32>,
    drop: Vec<u32>,
    raw: &str,
) -> CloseSelection {
    sel_drop_log(in_progress, complete, drop, Vec::new(), raw)
}

fn sel_log(
    in_progress: Option<Vec<u32>>,
    complete: Vec<u32>,
    log: Vec<(u32, &str)>,
    raw: &str,
) -> CloseSelection {
    sel_drop_log(in_progress, complete, Vec::new(), log, raw)
}

fn sel_drop_log(
    in_progress: Option<Vec<u32>>,
    complete: Vec<u32>,
    drop: Vec<u32>,
    log: Vec<(u32, &str)>,
    raw: &str,
) -> CloseSelection {
    CloseSelection {
        in_progress: in_progress
            .map(|list| list.into_iter().collect::<BTreeSet<u32>>()),
        complete: complete.into_iter().collect::<BTreeSet<u32>>(),
        drop: drop.into_iter().collect::<BTreeSet<u32>>(),
        log: log
            .into_iter()
            .map(|(index, text)| {
                super::super::capture_language::CloseLogEntry {
                    index,
                    text: text.to_string(),
                }
            })
            .collect(),
        raw: raw.to_string(),
    }
}

#[test]
fn numbers_the_worked_example() {
    let contents = worked_example();
    let running = running_of(&contents);
    assert_eq!(running.line, 5);
    let links = number_task_links(&contents, &running);
    assert_eq!(links.len(), 2);
    assert_eq!(links[0].index, 1);
    assert_eq!(links[0].line, 6);
    assert_eq!(links[0].block_link, "[[bob#^capture-stop]]");
    assert_eq!(links[0].path_part, "bob");
    assert_eq!(links[0].block_id, "capture-stop");
    assert_eq!(links[0].marker, TaskLinkMarker::Plain);
    assert_eq!(links[0].outcome, TaskLinkOutcome::InProgress);
    assert_eq!(links[0].source, TaskLinkSource::Ledger);
    assert_eq!(links[1].index, 2);
    assert_eq!(links[1].line, 10);
    assert_eq!(links[1].block_link, "[[bob#^web-capture]]");
    assert_eq!(links[1].marker, TaskLinkMarker::Deferred);
    assert_eq!(links[1].outcome, TaskLinkOutcome::Deferred);
    assert_eq!(links[1].source, TaskLinkSource::Ledger);
}

#[test]
fn nested_bare_links_are_numbered() {
    let contents = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "\t- [[bob#^parent]]",
        "\t\t- [[bob#^child]]",
        "\t\t\t- [[bob#^grandchild]]#",
    ]);
    let running = running_of(&contents);
    let links = number_task_links(&contents, &running);
    assert_eq!(
        links
            .iter()
            .map(|link| (link.index, link.line, link.block_id.as_str()))
            .collect::<Vec<_>>(),
        vec![(1, 3, "parent"), (2, 4, "child"), (3, 5, "grandchild")]
    );
    assert_eq!(links[2].marker, TaskLinkMarker::Deferred);
}

#[test]
fn embedded_hash_alias_and_tomato() {
    let contents = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "\t- ![[bob#^emb]]#",
        "\t- [[bob#^aliased|Alias]]",
        "\t- 🍅 [[bob#^tomato]]",
        "\t- 🍅 🍅 [[bob#^double]]",
    ]);
    let running = running_of(&contents);
    let links = number_task_links(&contents, &running);
    let ids: Vec<&str> =
        links.iter().map(|link| link.block_id.as_str()).collect();
    assert_eq!(ids, vec!["emb", "aliased", "tomato", "double"]);
    assert_eq!(links[0].marker, TaskLinkMarker::Embedded);
    assert_eq!(
        links[0].block_link, "[[bob#^emb]]",
        "block_link never includes `!`"
    );
    assert_eq!(links[1].block_link, "[[bob#^aliased|Alias]]");
    assert_eq!(links[1].marker, TaskLinkMarker::Plain);
}

#[test]
fn fenced_links_are_unnumbered() {
    // An indented fence inside the running Pomodoro's sub-bullet range: the
    // fenced link is never numbered. The fence markers themselves end the
    // sub-bullet range, so anything after the fence stays outside the range.
    let contents = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "\t- [[bob#^keep]]",
        "\t```md",
        "\t- [[bob#^fenced]]#",
        "\t```",
        "\t- [[bob#^after]]",
    ]);
    let running = running_of(&contents);
    let links = number_task_links(&contents, &running);
    assert_eq!(
        links
            .iter()
            .map(|l| l.block_id.as_str())
            .collect::<Vec<_>>(),
        vec!["keep"],
        "indented fenced and post-fence links are never numbered"
    );
}

#[test]
fn mixed_lines_are_unnumbered() {
    let contents = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "\t- prose [[bob#^mixed]]",
        "\t- [[bob#^a]] [[bob#^b]]",
        "\t- [[bob#^spaced]] #",
        "\t- ![[bob#^emb]] extra",
        "\t- ~~[[bob#^struck]]~~",
        "\t- quick note",
        "\t- [[bob#^good]]",
    ]);
    let running = running_of(&contents);
    let links = number_task_links(&contents, &running);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].block_id, "good");
}

#[test]
fn outcome_table_covers_every_row() {
    let contents = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "\t- [[a#^one]]",
        "\t- [[a#^two]]#",
        "\t- ![[a#^three]]",
    ]);
    let running = running_of(&contents);
    // Listed complete beats everything.
    let applied =
        apply_close_selection(&contents, &running, &sel(None, vec![2], "=x!2"))
            .expect("apply");
    assert_eq!(
        applied
            .lineup
            .iter()
            .map(|l| (l.index, l.outcome, l.source))
            .collect::<Vec<_>>(),
        vec![
            (1, TaskLinkOutcome::InProgress, TaskLinkSource::Ledger),
            (2, TaskLinkOutcome::Complete, TaskLinkSource::Listed),
            (3, TaskLinkOutcome::Complete, TaskLinkSource::Ledger),
        ]
    );
    // Listed in-progress beats ledger.
    let applied = apply_close_selection(
        &contents,
        &running,
        &sel(Some(vec![2]), vec![], "=x2"),
    )
    .expect("apply");
    assert_eq!(
        applied
            .lineup
            .iter()
            .map(|l| (l.index, l.outcome, l.source))
            .collect::<Vec<_>>(),
        vec![
            (1, TaskLinkOutcome::Deferred, TaskLinkSource::Unlisted),
            (2, TaskLinkOutcome::InProgress, TaskLinkSource::Listed),
            (3, TaskLinkOutcome::Complete, TaskLinkSource::Unlisted),
        ],
        "hand transclusion is kept when <N> is typed"
    );
    // No list: ledger outcomes and sources.
    let applied =
        apply_close_selection(&contents, &running, &sel(None, vec![], "=x"))
            .expect("apply");
    assert_eq!(
        applied
            .lineup
            .iter()
            .map(|l| (l.index, l.outcome, l.source))
            .collect::<Vec<_>>(),
        vec![
            (1, TaskLinkOutcome::InProgress, TaskLinkSource::Ledger),
            (2, TaskLinkOutcome::Deferred, TaskLinkSource::Ledger),
            (3, TaskLinkOutcome::Complete, TaskLinkSource::Ledger),
        ]
    );
    // Empty <N> (=x0): everything unlisted.
    let applied = apply_close_selection(
        &contents,
        &running,
        &sel(Some(vec![]), vec![], "=x0"),
    )
    .expect("apply");
    assert_eq!(
        applied
            .lineup
            .iter()
            .map(|l| (l.index, l.outcome, l.source))
            .collect::<Vec<_>>(),
        vec![
            (1, TaskLinkOutcome::Deferred, TaskLinkSource::Unlisted),
            (2, TaskLinkOutcome::Deferred, TaskLinkSource::Unlisted),
            (3, TaskLinkOutcome::Complete, TaskLinkSource::Unlisted),
        ]
    );
}

#[test]
fn drop_removes_from_closed_session_without_carry() {
    let contents = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "\t- [[a#^one]]",
        "\t- [[a#^two]]",
        "\t- [[a#^three]]#",
    ]);
    let running = running_of(&contents);
    let applied = apply_close_selection(
        &contents,
        &running,
        &sel_drop(None, vec![], vec![2], "=x~2"),
    )
    .expect("apply");
    assert_eq!(
        applied
            .lineup
            .iter()
            .map(|l| (l.index, l.outcome, l.source))
            .collect::<Vec<_>>(),
        vec![
            (1, TaskLinkOutcome::InProgress, TaskLinkSource::Ledger),
            (2, TaskLinkOutcome::Dropped, TaskLinkSource::Listed),
            (3, TaskLinkOutcome::Deferred, TaskLinkSource::Ledger),
        ]
    );
    assert!(
        applied.contents.contains("\t- ~[[a#^two]]"),
        "{}",
        applied.contents
    );
    let plan = plan_ledger_close(&applied.contents, &running, at(9, 37));
    // The dropped line leaves the closed session entirely: no transient
    // marker and no carried copy.
    assert!(!plan.contents.contains('~'), "{}", plan.contents);
    assert!(!plan.contents.contains("two"), "{}", plan.contents);
    assert!(
        plan.classified_links
            .iter()
            .any(|link| link.block_id == "two" && !link.carried),
        "{:?}",
        plan.classified_links
    );
    assert_eq!(plan.carried_lines.len(), 2);
    assert!(
        plan.carried_lines.iter().all(|line| !line.contains("two")),
        "{:?}",
        plan.carried_lines
    );
    // An out-of-range drop number is a precise-range error.
    let error = apply_close_selection(
        &contents,
        &running,
        &sel_drop(None, vec![], vec![9], "=x~9"),
    )
    .expect_err("out of range drop");
    assert_eq!(
        error.to_string(),
        "`=x~9` names task 9, but CAPTURE has 3 numbered Task Links (1–3)"
    );
}

#[test]
fn none_is_byte_identical_to_plan_ledger_close() {
    let contents = worked_example();
    let running = running_of(&contents);
    let ledger = plan_ledger_close(&contents, &running, at(9, 37));
    let applied =
        apply_close_selection(&contents, &running, &sel(None, vec![], "=x"))
            .expect("apply with no lists");
    assert_eq!(applied.contents, contents);
    assert!(applied
        .lineup
        .iter()
        .all(|l| l.source == TaskLinkSource::Ledger));
    let relined = plan_ledger_close(&applied.contents, &running, at(9, 37));
    assert_eq!(relined.contents, ledger.contents);
    assert_eq!(relined.carried_lines, ledger.carried_lines);
}

#[test]
fn rewrite_keeps_prefix_and_drops_tomato() {
    let contents = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "\t- 🍅 [[a#^one]]",
        "\t- [[a#^two]]#",
    ]);
    let running = running_of(&contents);
    // Defer 1 (was plain): drops the tomato, appends `#`.
    let applied = apply_close_selection(
        &contents,
        &running,
        &sel(Some(vec![2]), vec![], "=x2"),
    )
    .expect("apply");
    assert!(applied.contents.contains("\t- [[a#^one]]#\n"));
    assert!(!applied.contents.contains("🍅 [[a#^one]]"));
    assert!(applied.contents.contains("\t- [[a#^two]]\n"));
    // Unchanged lines stay byte-identical.
    let applied = apply_close_selection(
        &contents,
        &running,
        &sel(Some(vec![1]), vec![], "=x1"),
    )
    .expect("apply");
    // Line 2 was already deferred-adjacent? 1 stays plain, 2 stays
    // deferred-ledged except 1 listed keeps plain: only line 4 changes?
    // Here =x1 equals the ledger (1 plain, 2 deferred) so nothing changes.
    assert_eq!(applied.contents, contents);
}

#[test]
fn apply_preserves_crlf_and_missing_final_newline() {
    let contents = worked_example().replace('\n', "\r\n");
    let running = running_of(&contents);
    let applied = apply_close_selection(
        &contents,
        &running,
        &sel(Some(vec![2]), vec![], "=x2"),
    )
    .expect("apply");
    assert!(applied.contents.contains("\r\n"));
    assert!(!applied.contents.replace("\r\n", "").contains('\n'));
    assert!(applied.contents.ends_with("\r\n"));

    let mut no_final = worked_example();
    assert_eq!(no_final.pop(), Some('\n'));
    let running = running_of(&no_final);
    let applied = apply_close_selection(
        &no_final,
        &running,
        &sel(Some(vec![2]), vec![], "=x2"),
    )
    .expect("apply");
    assert!(!applied.contents.ends_with('\n'));
}

#[test]
fn out_of_range_messages() {
    let contents = worked_example();
    let running = running_of(&contents);
    // One bad number with two links.
    let error = apply_close_selection(
        &contents,
        &running,
        &sel(Some(vec![4]), vec![], "=x4"),
    )
    .expect_err("out of range");
    assert_eq!(
        error.to_string(),
        "`=x4` names task 4, but CAPTURE has 2 numbered Task Links (1–2)"
    );
    // Multiple bad numbers list every one.
    let error = apply_close_selection(
        &contents,
        &running,
        &sel(Some(vec![4, 6]), vec![], "=x4,6"),
    )
    .expect_err("out of range");
    assert_eq!(
        error.to_string(),
        "`=x4,6` names tasks 4 and 6, but CAPTURE has 2 numbered Task Links (1–2)"
    );
    // Complete-list numbers validate too.
    let error =
        apply_close_selection(&contents, &running, &sel(None, vec![3], "=x!3"))
            .expect_err("out of range");
    assert_eq!(
        error.to_string(),
        "`=x!3` names task 3, but CAPTURE has 2 numbered Task Links (1–2)"
    );
    // `=x0` never ranges.
    apply_close_selection(
        &contents,
        &running,
        &sel(Some(vec![]), vec![], "=x0"),
    )
    .expect("zero is always in range");
}

#[test]
fn out_of_range_with_one_and_zero_links() {
    let one = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m]) — SOLO",
        "\t- [[a#^only]]",
    ]);
    let running = running_of(&one);
    let error =
        apply_close_selection(&one, &running, &sel(None, vec![2], "=x!2"))
            .expect_err("out of range");
    assert_eq!(
        error.to_string(),
        "`=x!2` names task 2, but SOLO has 1 numbered Task Link (1)"
    );

    let none = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "\t- quick note",
    ]);
    let running = running_of(&none);
    let error =
        apply_close_selection(&none, &running, &sel(None, vec![1], "=x1"))
            .expect_err("out of range");
    assert_eq!(
        error.to_string(),
        "`=x1` names task 1, but CAPTURE has no numbered Task Links; close it with `=x`"
    );

    let unnamed = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m])",
        "\t- [[a#^one]]",
    ]);
    let running = running_of(&unnamed);
    let error = apply_close_selection(
        &unnamed,
        &running,
        &sel(Some(vec![2]), vec![], "=x2"),
    )
    .expect_err("out of range");
    assert_eq!(
        error.to_string(),
        "`=x2` names task 2, but the running Pomodoro has 1 numbered Task Link (1)"
    );
}

#[test]
fn conflicting_duplicates_fail_and_same_outcome_passes() {
    let contents = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "\t- [[bob#^a]]",
        "\t- [[bob#^b]]",
        "\t- [[bob#^a]]",
    ]);
    let running = running_of(&contents);
    // Same outcome on both copies is fine.
    apply_close_selection(
        &contents,
        &running,
        &sel(Some(vec![1, 3]), vec![], "=x1,3"),
    )
    .expect("same outcome duplicates pass");
    // Different outcomes conflict.
    let error = apply_close_selection(
        &contents,
        &running,
        &sel(Some(vec![1]), vec![], "=x1"),
    )
    .expect_err("conflicting duplicate");
    match &error {
        CloseSelectionError::ConflictingDuplicate {
            indices,
            block_link,
        } => {
            assert_eq!(indices, &vec![1, 3]);
            assert_eq!(block_link, "[[bob#^a]]");
        }
        other => panic!("expected conflicting duplicate, got {other:?}"),
    }
    assert_eq!(
        error.to_string(),
        "tasks 1 and 3 both link `[[bob#^a]]` but get different outcomes; give them the same one"
    );
}

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

    fn worked() -> Self {
        let mut vault = Self::new();
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
        vault
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

fn close_with(
    vault: &MemoryVault,
    selection: Option<&CloseSelection>,
) -> super::linked_tasks::PomodoroClosePlan {
    let day_path = Path::new("2026/20260928.md");
    plan_pomodoro_close(
        day_path,
        &worked_example(),
        at(9, 37),
        vault,
        selection,
    )
    .expect("close plan")
}

fn closed_ledger_tail(plan: &super::linked_tasks::PomodoroClosePlan) -> String {
    // From the closed CAPTURE entry through the placeholder, matching the
    // plan's worked-example blocks (tabs preserved).
    let contents = &plan.summary.ledger.contents;
    let marker = "- [x] (**0920-0940** [t:: 20m]) — CAPTURE\n";
    let start = contents.find(marker).expect("closed entry");
    contents[start..].to_string()
}

#[test]
fn full_close_reports_numbered_lineup_with_none() {
    let vault = MemoryVault::worked();
    let plan = close_with(&vault, None);
    assert_eq!(plan.summary.task_links.len(), 2);
    assert_eq!(plan.summary.task_links[0].index, 1);
    assert_eq!(plan.summary.task_links[0].source, TaskLinkSource::Ledger);
    assert_eq!(plan.summary.task_links[1].index, 2);
    assert_eq!(plan.summary.task_links[1].source, TaskLinkSource::Ledger);
    let indices: Vec<Option<u32>> =
        plan.summary.tasks.iter().map(|task| task.index).collect();
    // Worked, deferred, struck rows: first two numbered, struck unnumbered.
    assert_eq!(indices, vec![Some(1), Some(2), None]);
    assert!(plan.warnings.is_empty());
}

#[test]
fn ledger_post_image_for_x2() {
    let vault = MemoryVault::worked();
    let selection = sel(Some(vec![2]), vec![], "=x2");
    let plan = close_with(&vault, Some(&selection));
    assert_eq!(
        plan.summary
            .task_links
            .iter()
            .map(|l| (l.index, l.outcome, l.source))
            .collect::<Vec<_>>(),
        vec![
            (1, TaskLinkOutcome::Deferred, TaskLinkSource::Unlisted),
            (2, TaskLinkOutcome::InProgress, TaskLinkSource::Listed),
        ]
    );
    // Deferred first link leaves its notes orphaned under the closed entry,
    // exactly as the matching hand edits followed by `=x` produce.
    assert_eq!(
        closed_ledger_tail(&plan),
        note(&[
            "- [x] (**0920-0940** [t:: 20m]) — CAPTURE",
            "\t\t- Designed the `=x` grammar",
            "\t\t\t- chose `x` for done",
            "\t\t- Wrote the plan",
            "\t- 🍅 [[bob#^web-capture]]",
            "\t- ~~[[sase#^axe-restart]]~~",
            "\t\t- Restarted axe",
            "\t- quick note",
            "- [ ] () — CAPTURE",
            "\t- [[bob#^web-capture]]",
            "\t- [[bob#^capture-stop]]",
            "- [ ] () — SASE",
            "\t- [[sase#^recovery-panel]]",
        ])
    );
    let next = plan.summary.ledger.next_pomodoro.expect("next");
    assert_eq!(next.line, 13);
    assert!(next.created);
}

#[test]
fn ledger_post_image_for_x_complete() {
    let vault = MemoryVault::worked();
    let plan = close_with(&vault, Some(&sel(None, vec![1], "=x!1")));
    assert_eq!(
        closed_ledger_tail(&plan),
        note(&[
            "- [x] (**0920-0940** [t:: 20m]) — CAPTURE",
            "\t- ~~[[bob#^capture-stop]]~~",
            "\t\t- Designed the `=x` grammar",
            "\t\t\t- chose `x` for done",
            "\t\t- Wrote the plan",
            "\t- ~~[[sase#^axe-restart]]~~",
            "\t\t- Restarted axe",
            "\t- quick note",
            "- [ ] () — CAPTURE",
            "\t- [[bob#^web-capture]]",
            "- [ ] () — SASE",
            "\t- [[sase#^recovery-panel]]",
        ])
    );
}

#[test]
fn ledger_post_image_for_x1_complete_2() {
    let vault = MemoryVault::worked();
    let plan = close_with(&vault, Some(&sel(Some(vec![1]), vec![2], "=x1!2")));
    assert_eq!(
        closed_ledger_tail(&plan),
        note(&[
            "- [x] (**0920-0940** [t:: 20m]) — CAPTURE",
            "\t- 🍅 [[bob#^capture-stop]]",
            "\t\t- Designed the `=x` grammar",
            "\t\t\t- chose `x` for done",
            "\t\t- Wrote the plan",
            "\t- ~~[[bob#^web-capture]]~~",
            "\t- ~~[[sase#^axe-restart]]~~",
            "\t\t- Restarted axe",
            "\t- quick note",
            "- [ ] () — CAPTURE",
            "\t- [[bob#^capture-stop]]",
            "- [ ] () — SASE",
            "\t- [[sase#^recovery-panel]]",
        ])
    );
    let next = plan.summary.ledger.next_pomodoro.expect("next");
    assert_eq!(next.line, 14);
}

#[test]
fn ledger_post_image_for_x0_and_x1_2() {
    let vault = MemoryVault::worked();
    let plan = close_with(&vault, Some(&sel(Some(vec![]), vec![], "=x0")));
    assert_eq!(
        closed_ledger_tail(&plan),
        note(&[
            "- [x] (**0920-0940** [t:: 20m]) — CAPTURE",
            "\t\t- Designed the `=x` grammar",
            "\t\t\t- chose `x` for done",
            "\t\t- Wrote the plan",
            "\t- ~~[[sase#^axe-restart]]~~",
            "\t\t- Restarted axe",
            "\t- quick note",
            "- [ ] () — CAPTURE",
            "\t- [[bob#^capture-stop]]",
            "\t- [[bob#^web-capture]]",
            "- [ ] () — SASE",
            "\t- [[sase#^recovery-panel]]",
        ])
    );
    let next = plan.summary.ledger.next_pomodoro.expect("next");
    assert_eq!(next.line, 12);

    let plan =
        close_with(&vault, Some(&sel(Some(vec![1, 2]), vec![], "=x1,2")));
    assert!(plan
        .summary
        .ledger
        .contents
        .contains("\t- 🍅 [[bob#^capture-stop]]"));
    assert!(plan
        .summary
        .ledger
        .contents
        .contains("\t- 🍅 [[bob#^web-capture]]"));
    let placeholder = plan.summary.ledger.next_pomodoro.expect("next");
    assert!(placeholder.created);
}

#[test]
fn listed_blocked_and_done_warnings() {
    let mut vault = MemoryVault::new();
    let settings_dir =
        vault.root.join(".obsidian/plugins/obsidian-tasks-plugin");
    std::fs::create_dir_all(&settings_dir).expect("settings dir");
    std::fs::write(
        settings_dir.join("data.json"),
        r##"{"globalFilter": "#task","statusSettings": {"coreStatuses": [{"symbol":" ","name":"Ready","type":"TODO"},{"symbol":"x","name":"Done","type":"DONE"}],"customStatuses": [{"symbol":"*","name":"Next","type":"ON_HOLD"},{"symbol":"?","name":"Blocked","type":"TODO"},{"symbol":"/","name":"In Progress","type":"IN_PROGRESS"},{"symbol":"-","name":"Canceled","type":"CANCELLED"}]}}"##,
    )
    .expect("settings file");
    vault.insert(
        "tasks.md",
        concat!("- [?] #task Blocked ^blocked\n", "- [x] #task Done ^done\n",),
    );
    let day = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "\t- [[tasks#^blocked]]",
        "\t- [[tasks#^done]]",
    ]);
    let day_path = Path::new("2026/20260928.md");
    // 2 stays in progress (Done cannot start) and 1 completes (Blocked
    // cannot complete).
    let plan = plan_pomodoro_close(
        day_path,
        &day,
        at(9, 37),
        &vault,
        Some(&sel(Some(vec![2]), vec![1], "=x2!1")),
    )
    .expect("close plan");
    assert_eq!(plan.summary.tasks.len(), 2);
    assert_eq!(plan.summary.tasks[0].index, Some(1));
    assert_eq!(plan.summary.tasks[1].index, Some(2));
    assert_eq!(
        plan.summary.tasks[0].warning.as_deref(),
        Some("task 1 `[[tasks#^blocked]]` is Blocked, so it was not completed")
    );
    assert_eq!(
        plan.summary.tasks[1].warning.as_deref(),
        Some("task 2 `[[tasks#^done]]` is Done, so it was not started")
    );
    assert!(plan.warnings.iter().any(|w| w.contains("was not started")));
    assert!(plan
        .warnings
        .iter()
        .any(|w| w.contains("was not completed")));
}

#[test]
fn log_entry_appends_after_existing_descendants() {
    let contents = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "\t- [[a#^one]]",
        "\t\t- hand note",
        "\t\t\t- grandchild",
        "\t- [[a#^two]]",
    ]);
    let running = running_of(&contents);
    let applied = apply_close_selection(
        &contents,
        &running,
        &sel_log(None, vec![], vec![(1, "wired the lexer")], "=x"),
    )
    .expect("apply");
    assert_eq!(
        applied.contents,
        note(&[
            "## Pomodoros",
            "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
            "\t- [[a#^one]]",
            "\t\t- hand note",
            "\t\t\t- grandchild",
            "\t\t- wired the lexer",
            "\t- [[a#^two]]",
        ])
    );
    assert_eq!(applied.inserted_lines, vec![6]);
    assert_eq!(
        applied
            .lineup
            .iter()
            .map(|link| (link.index, link.line))
            .collect::<Vec<_>>(),
        vec![(1, 3), (2, 7)]
    );
}

#[test]
fn log_entry_child_indent_follows_first_child_or_link_indent() {
    // A link with no children and a two-space indent doubles the indent.
    let contents = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "  - [[a#^one]]",
        "  - [[a#^two]]",
    ]);
    let running = running_of(&contents);
    let applied = apply_close_selection(
        &contents,
        &running,
        &sel_log(None, vec![], vec![(2, "did it")], "=x"),
    )
    .expect("apply");
    assert!(applied.contents.contains("  - [[a#^two]]\n    - did it\n"));

    // A tab-indented link with no children adds one tab.
    let contents = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "\t- [[a#^one]]",
    ]);
    let running = running_of(&contents);
    let applied = apply_close_selection(
        &contents,
        &running,
        &sel_log(None, vec![], vec![(1, "did it")], "=x"),
    )
    .expect("apply");
    assert!(applied.contents.contains("\t- [[a#^one]]\n\t\t- did it\n"));
}

#[test]
fn log_entries_keep_typed_order_for_repeated_index() {
    let contents = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "\t- [[a#^one]]",
        "\t\t- hand note",
    ]);
    let running = running_of(&contents);
    let applied = apply_close_selection(
        &contents,
        &running,
        &sel_log(None, vec![], vec![(1, "first"), (1, "second")], "=x"),
    )
    .expect("apply");
    assert_eq!(
        applied.contents,
        note(&[
            "## Pomodoros",
            "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
            "\t- [[a#^one]]",
            "\t\t- hand note",
            "\t\t- first",
            "\t\t- second",
        ])
    );
    assert_eq!(applied.inserted_lines, vec![5, 6]);
}

#[test]
fn log_entry_preserves_crlf_and_missing_final_newline() {
    let contents = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "\t- [[a#^one]]",
    ])
    .replace('\n', "\r\n");
    let running = running_of(&contents);
    let applied = apply_close_selection(
        &contents,
        &running,
        &sel_log(None, vec![], vec![(1, "did it")], "=x"),
    )
    .expect("apply");
    assert!(applied.contents.contains("\t\t- did it\r\n"));
    assert!(!applied.contents.replace("\r\n", "").contains('\n'));

    let mut no_final = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "\t- [[a#^one]]",
    ]);
    no_final.pop();
    let running = running_of(&no_final);
    let applied = apply_close_selection(
        &no_final,
        &running,
        &sel_log(None, vec![], vec![(1, "did it")], "=x"),
    )
    .expect("apply");
    assert!(applied.contents.ends_with("\t\t- did it"));
    assert!(!applied.contents.ends_with('\n'));
}

#[test]
fn log_entry_errors() {
    let contents = worked_example();
    let running = running_of(&contents);
    let error = apply_close_selection(
        &contents,
        &running,
        &sel_log(None, vec![], vec![(5, "x")], "=x"),
    )
    .expect_err("out of range entry");
    assert_eq!(
        error.to_string(),
        "`=x` logs to task 5, but CAPTURE has 2 numbered Task Links (1–2); write `\\5` to keep the number as text"
    );

    let error = apply_close_selection(
        &contents,
        &running,
        &sel_log(None, vec![], vec![(2, "x")], "=x"),
    )
    .expect_err("deferred entry");
    assert_eq!(
        error.to_string(),
        "task 2 `[[bob#^web-capture]]` is deferred, so it can't take a Work Log entry; list it in `<N>` or `!<M>` to log to it"
    );

    let error = apply_close_selection(
        &contents,
        &running,
        &sel_drop_log(None, vec![], vec![2], vec![(2, "x")], "=x~2"),
    )
    .expect_err("dropped entry");
    assert_eq!(
        error.to_string(),
        "task 2 `[[bob#^web-capture]]` is dropped, so it can't take a Work Log entry"
    );

    let nested = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "\t- [[bob#^parent]]",
        "\t\t- [[bob#^child]]",
    ]);
    let running = running_of(&nested);
    let error = apply_close_selection(
        &nested,
        &running,
        &sel_log(None, vec![], vec![(2, "x")], "=x"),
    )
    .expect_err("nested entry");
    assert_eq!(
        error.to_string(),
        "task 2 `[[bob#^child]]` is nested under another bullet, so the close can't write its Work Log; move it to the top level of CAPTURE"
    );
}

#[test]
fn log_entry_text_that_numbers_a_link_fails_loudly() {
    let contents = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "\t- [[a#^one]]",
    ]);
    let running = running_of(&contents);
    let error = apply_close_selection(
        &contents,
        &running,
        &sel_log(None, vec![], vec![(1, "[[a#^evil]]")], "=x"),
    )
    .expect_err("lineup guard");
    assert_eq!(
        error.to_string(),
        "Work Log entry changed the Task Link lineup"
    );
}

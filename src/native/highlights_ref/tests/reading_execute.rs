//! v2-execution tests: guarded inserts, preferred reopen IDs, line edits,
//! revalidation, the v2 ref-note guard, and fail-before-write ordering.
//! Temp-vault throughout; dates are literals, never the clock.
use super::*;
use crate::native::ref_tasks::{
    LocatedRefTask, RefTaskIndex, READING_TASK_CHANGED,
};

fn insert_inputs<'a>(
    bob_dir: &'a Path,
    route: &'a str,
    mark: char,
    prefer: Option<&'a str>,
) -> ReadingInsertInputs<'a> {
    ReadingInsertInputs {
        bob_dir,
        destination_route: route,
        ref_target: "ref/papers/stem",
        ref_stem: "stem",
        title_raw: "Stem",
        created: "2026-10-09",
        mark,
        warning_child: None,
        prefer_block_id: prefer,
    }
}

fn located_task(
    path: &str,
    line_index: usize,
    line: &str,
    mark: char,
    block_id: Option<&str>,
    archived: bool,
) -> LocatedRefTask {
    LocatedRefTask {
        path: path.to_string(),
        line_index,
        line: line.to_string(),
        mark,
        block_id: block_id.map(str::to_string),
        archived,
        residence: Some("sase".to_string()),
        closed_on: None,
        in_capture_target: true,
    }
}

#[test]
fn birth_insert_then_rerun_adopts_without_duplication() {
    let bob = temp_bob_dir("reading-execute-adopt");
    let ref_dir = bob.join("ref");
    write_test_file(&bob.join("sase.md"), "# Sase\n");
    let execution =
        execute_reading_insert(insert_inputs(&bob, "sase", ' ', None))
            .expect("birth insert");
    assert_eq!(execution.action, ReadingTaskExecutionAction::Inserted);
    assert_eq!(execution.destination, bob.join("sase.md"));
    // A rerun after the parent write succeeded must adopt the existing
    // task: the locator finds exactly this line under the ref key.
    let index = RefTaskIndex::build(&bob, &ref_dir);
    let candidates = index.candidates("ref/papers/stem.md");
    assert_eq!(candidates.len(), 1, "{candidates:?}");
    assert_eq!(candidates[0].block_id, Some(execution.block_id.clone()));
    let (birth, _) =
        crate::native::highlights_ref::select_birth_task(candidates);
    match birth {
        BirthTask::Adopt(located) => {
            assert_eq!(located.block_id, Some(execution.block_id.clone()));
            assert_eq!(located.line, execution.task_line);
        }
        other => panic!("rerun must adopt, got {other:?}"),
    }
}

#[test]
fn reopen_prefers_old_id_and_suffixes_on_collision() {
    let bob = temp_bob_dir("reading-execute-reopen");
    write_test_file(&bob.join("sase.md"), "# Sase\n");
    let first = execute_reading_insert(insert_inputs(
        &bob,
        "sase",
        ' ',
        Some("ref-essay"),
    ))
    .expect("reopen insert");
    assert_eq!(first.block_id, "ref-essay");
    assert!(
        first.task_line.ends_with("^ref-essay"),
        "{}",
        first.task_line
    );
    // The old address is taken now: the next reopen keeps the family with a
    // suffix, and the earlier line is untouched.
    let second = execute_reading_insert(insert_inputs(
        &bob,
        "sase",
        ' ',
        Some("ref-essay"),
    ))
    .expect("second reopen insert");
    assert_eq!(second.block_id, "ref-essay-2");
    let contents =
        fs::read_to_string(bob.join("sase.md")).expect("read parent");
    assert!(
        contents.contains("^ref-essay\n") || contents.contains("^ref-essay\r"),
        "{contents}"
    );
    assert!(contents.contains("^ref-essay-2"), "{contents}");
}

#[test]
fn reopen_into_inbox_fallback_carries_warning_child() {
    let bob = temp_bob_dir("reading-execute-inbox");
    let mut inputs = insert_inputs(&bob, "mac_inbox", ' ', None);
    inputs.warning_child =
        Some(parent_fallback_child("gone-parent").to_string());
    let execution = execute_reading_insert(inputs).expect("inbox insert");
    assert_eq!(execution.destination, bob.join("mac_inbox.md"));
    let contents =
        fs::read_to_string(&execution.destination).expect("read inbox");
    assert!(contents.contains("parent 'gone-parent'"), "{contents}");
    assert!(
        contents.contains("refile me with Ctrl+Shift+M"),
        "{contents}"
    );
}

#[test]
fn line_edit_updates_checkbox_and_stamps_close() {
    let bob = temp_bob_dir("reading-execute-edit");
    write_test_file(&bob.join("sase.md"), "# Sase\n");
    let inserted =
        execute_reading_insert(insert_inputs(&bob, "sase", ' ', None))
            .expect("birth insert");
    let task = located_task(
        "sase.md",
        2,
        &inserted.task_line,
        ' ',
        Some(&inserted.block_id),
        false,
    );
    // Revalidation runs before the edit in the real order: fresh bytes
    // still match the plan, so the address is confirmed for the ref note.
    let refreshed = revalidate_located_task(&bob, &task).expect("revalidate");
    assert_eq!(refreshed.mark, ' ');
    assert_eq!(refreshed.line, inserted.task_line);
    let edited =
        execute_reading_line_edit(&bob, &task, 'x').expect("line edit");
    assert_eq!(edited.action, ReadingTaskExecutionAction::LineEdited);
    assert_eq!(edited.block_id, inserted.block_id);
    assert!(
        edited.task_line.starts_with("- [x]"),
        "{}",
        edited.task_line
    );
    assert!(
        edited.task_line.contains("[completion:: 2026-10-09]"),
        "{}",
        edited.task_line
    );
    // After the edit the old bytes are (correctly) stale.
    let stale = revalidate_located_task(&bob, &task)
        .expect_err("old bytes are stale after the edit");
    assert_eq!(stale.message, READING_TASK_CHANGED);
}

#[test]
fn line_edit_refuses_archived_tasks() {
    let bob = temp_bob_dir("reading-execute-archive");
    let task = located_task(
        "done/sase_done.md",
        0,
        "- [x] #task #ref [[ref/papers/stem|Stem]] [created::2026-10-01] ^ref-stem",
        'x',
        Some("ref-stem"),
        true,
    );
    let error = execute_reading_line_edit(&bob, &task, ' ')
        .expect_err("archived edits must fail");
    assert!(error.message.contains("archived"), "{}", error.message);
}

#[test]
fn revalidate_refreshes_moved_lines_and_refuses_changed_ones() {
    let bob = temp_bob_dir("reading-execute-revalidate");
    let line = "- [ ] #task #ref [[ref/papers/stem|Stem]] [created::2026-10-09] ^ref-stem";
    write_test_file(&bob.join("sase.md"), &format!("# Sase\n\n{line}\n"));
    let task = located_task("sase.md", 2, line, ' ', Some("ref-stem"), false);
    // Unrelated lines above move the task down; the content is still unique.
    write_test_file(
        &bob.join("sase.md"),
        &format!("# Sase\n\n- [ ] other\n{line}\n"),
    );
    let refreshed =
        revalidate_located_task(&bob, &task).expect("moved revalidates");
    assert_eq!(refreshed.line_index, 3);
    assert_eq!(refreshed.block_id, Some("ref-stem".to_string()));
    // An edited line no longer matches anywhere: no stale embed or parent.
    write_test_file(
        &bob.join("sase.md"),
        "# Sase\n\n- [ ] #task #ref [[ref/papers/stem|Stem]] retitled [created::2026-10-09] ^ref-stem\n",
    );
    let error =
        revalidate_located_task(&bob, &task).expect_err("changed must fail");
    assert_eq!(error.message, READING_TASK_CHANGED);
}

#[test]
fn changed_line_fails_before_any_pdf_write() {
    let bob = temp_bob_dir("reading-execute-order");
    let line = "- [ ] #task #ref [[ref/papers/stem|Stem]] [created::2026-10-09] ^ref-stem";
    write_test_file(&bob.join("sase.md"), &format!("# Sase\n\n{line}\n"));
    let ref_note = "# Stem\n\n![[sase#^ref-stem]]\n";
    write_test_file(&bob.join("ref/papers/stem.md"), ref_note);
    let task = located_task("sase.md", 2, line, ' ', Some("ref-stem"), false);
    // Someone edits the task line mid-sync; the executor fails first, so
    // the ref note (and any marker) for this PDF stays unwritten.
    write_test_file(
        &bob.join("sase.md"),
        "# Sase\n\n- [ ] #task #ref [[ref/papers/stem|Stem]] retitled [created::2026-10-09] ^ref-stem\n",
    );
    let error = execute_reading_line_edit(&bob, &task, 'x')
        .expect_err("changed line must fail");
    assert_eq!(error.message, READING_TASK_CHANGED);
    assert_eq!(
        fs::read_to_string(bob.join("ref/papers/stem.md")).expect("read"),
        ref_note
    );
}

fn v2_note(base_body: &str, parent: &str) -> String {
    format!(
        "---\nstatus: ready\nparent: \"[[{parent}]]\"\ntype: \"[[ref]]\"\n---\n{base_body}"
    )
}

#[test]
fn v2_guard_allows_embed_repoint_and_residence_parent() {
    let base = v2_note("# Stem\n\n![[sase#^ref-stem]]\n", "sase");
    // Repointed embed plus a residence-only parent move: allowed without a
    // frontmatter contribution.
    let current = v2_note("# Stem\n\n![[bob#^ref-stem]]\n", "bob");
    assert!(v2_dirty_note_allowed(&base, &current, false));
    // Deleted embed line: still confined to the managed slot (the blank
    // separator the deletion leaves behind is the slot's own residue).
    let deleted = v2_note("# Stem\n\n", "sase");
    assert!(v2_dirty_note_allowed(&base, &deleted, false));
}

#[test]
fn v2_guard_refuses_unrelated_edits() {
    let base = v2_note("# Stem\n\n![[sase#^ref-stem]]\n", "sase");
    // Authored body text changed outside the slot: refused.
    let edited =
        v2_note("# Stem\n\n![[sase#^ref-stem]]\n\nMy notes.\n", "sase");
    assert!(!v2_dirty_note_allowed(&base, &edited, false));
    // A v1-style checkbox flip on an in-note tracker is not a managed-embed
    // change: refused here, so the v1 guard keeps its exact allowance.
    let tracker_base =
        v2_note("# Stem\n\n- [ ] #task #pdf [[x.pdf]] #hide ^ref\n", "sase");
    let tracker_flipped =
        v2_note("# Stem\n\n- [x] #task #pdf [[x.pdf]] #hide ^ref\n", "sase");
    assert!(!v2_dirty_note_allowed(
        &tracker_base,
        &tracker_flipped,
        false
    ));
    // A non-residence frontmatter change without contribution: refused.
    let status_changed = v2_note("# Stem\n\n![[sase#^ref-stem]]\n", "sase")
        .replacen("status: ready", "status: wip", 1);
    assert!(!v2_dirty_note_allowed(&base, &status_changed, false));
    // ... but allowed when the sync itself contributed frontmatter.
    assert!(v2_dirty_note_allowed(&base, &status_changed, true));
}

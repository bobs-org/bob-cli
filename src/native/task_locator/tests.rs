use std::{collections::BTreeMap, path::Path, path::PathBuf};

use super::*;

fn settings() -> NoteTaskSettings {
    note_tasks::read_settings(Path::new("/no-bob-vault"))
}

#[test]
fn parses_both_supported_locator_forms_and_uses_last_colon() {
    assert_eq!(
        TaskLocator::parse("folder:sub:note:task-id"),
        Some(TaskLocator {
            note: "folder:sub:note".to_string(),
            block_id: "task-id".to_string(),
        })
    );
    assert_eq!(
        TaskLocator::parse("[[folder/note#^task-id]]"),
        Some(TaskLocator {
            note: "folder/note".to_string(),
            block_id: "task-id".to_string(),
        })
    );
}

#[test]
fn resolves_task_and_reports_one_based_line() {
    let snapshot = BTreeMap::from([(
        PathBuf::from("work/note.md"),
        "# Heading\n- [ ] #task Do it ^do-it\n".to_string(),
    )]);
    let task = resolve_task(&snapshot, "note:do-it", &settings())
        .expect("task should resolve");
    assert_eq!(task.path, PathBuf::from("work/note.md"));
    assert_eq!(task.line, 2);
    assert_eq!(task.task.description, "Do it");
}

#[test]
fn distinguishes_ambiguous_notes_non_tasks_and_missing_tasks() {
    let settings = settings();
    let duplicate_names = BTreeMap::from([
        (PathBuf::from("a/note.md"), "- [ ] #task A ^a\n".to_string()),
        (PathBuf::from("b/note.md"), "- [ ] #task B ^b\n".to_string()),
    ]);
    assert_eq!(
        resolve_task(&duplicate_names, "note:a", &settings)
            .expect_err("basename is ambiguous")
            .code,
        "ambiguous_note"
    );

    let non_task = BTreeMap::from([(
        PathBuf::from("note.md"),
        "- A plain bullet ^anchor\n".to_string(),
    )]);
    assert_eq!(
        resolve_task(&non_task, "note:anchor", &settings)
            .expect_err("anchor is not a task")
            .code,
        "not_a_task"
    );
    assert_eq!(
        resolve_task(&non_task, "note:missing", &settings)
            .expect_err("task is missing")
            .code,
        "task_not_found"
    );
}

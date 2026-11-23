use std::path::Path;

use super::*;

fn settings() -> NoteTaskSettings {
    note_tasks::read_settings(Path::new("/no-bob-vault"))
}

fn note(lines: &[&str]) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

fn tidied(contents: &str) -> NoteTidy {
    let pass = tidy_note(contents, &settings());
    if let Some(after) = &pass.updated {
        content_preserved(contents, after, pass.drawers_created())
            .expect("content preserved");
        settled(after, &settings()).expect("settled after one pass");
        assert_eq!(
            tidy_note(after, &settings()),
            NoteTidy {
                open_tasks: pass.open_tasks,
                updated: None,
                repairs: Vec::new(),
            },
            "a second pass reports nothing"
        );
    }
    pass
}

#[test]
fn a_canonical_note_plans_nothing() {
    let contents = note(&[
        "## Tasks",
        "- [ ] #task Ship the exporter ^export",
        "\t- ⛓️ **DEPENDS ON:** [[#^parser]]",
        "\t- [ ] Handles supported inputs",
        "\t\t- [ ] Handles Windows files",
        "\t- NOTES",
        "\t\t- Morgan reported the original issue.",
        "- [ ] #task Plain task",
        "\t- a loose note",
    ]);
    let pass = tidied(&contents);
    assert_eq!(pass.open_tasks, 2);
    assert_eq!(pass.updated, None);
    assert!(pass.repairs.is_empty());
}

#[test]
fn a_req_owning_task_files_loose_notes_under_new_notes() {
    let contents = note(&[
        "- [ ] #task Ship the exporter ^export",
        "\t- Morgan reported the original issue.",
        "\t- [ ] Handles supported inputs",
        "\t\t- Fixture: windows.csv",
        "\t- ⛓️ **DEPENDS ON:** [[#^parser]]",
    ]);
    let pass = tidied(&contents);
    assert_eq!(
        pass.updated.as_deref(),
        Some(
            note(&[
                "- [ ] #task Ship the exporter ^export",
                "\t- ⛓️ **DEPENDS ON:** [[#^parser]]",
                "\t- [ ] Handles supported inputs",
                "\t\t- Fixture: windows.csv",
                "\t- NOTES",
                "\t\t- Morgan reported the original issue.",
            ])
            .as_str()
        )
    );
    assert_eq!(pass.repairs.len(), 1);
    let repair = &pass.repairs[0];
    assert_eq!(repair.line_index, 0);
    assert_eq!(repair.text, "Ship the exporter");
    assert_eq!(repair.block_id.as_deref(), Some("export"));
    assert_eq!(repair.notes_filed, 1);
    assert!(repair.notes_created);
    assert!(repair.moves >= 1);
    assert_eq!(repair.before.len(), 5);
    assert_eq!(repair.after.len(), 6);
    assert_eq!(repair.after[4], "\t- NOTES");
}

#[test]
fn closed_tasks_are_history_and_never_tidied() {
    let contents = note(&[
        "- [x] #task Shipped ✅ 2026-11-20",
        "\t- 🛠️ **WORK LOG**",
        "\t\t- *2026-11-20* — done",
        "\t- a note after the log",
        "- [-] #task Dropped",
        "\t- a note",
        "\t- [ ] a req after the note",
    ]);
    let pass = tidied(&contents);
    assert_eq!(pass.open_tasks, 0);
    assert_eq!(pass.updated, None);
}

#[test]
fn a_task_with_no_reqs_only_moves_logs_and_links() {
    let contents = note(&[
        "- [ ] #task Plain task",
        "\t- 🛠️ **WORK LOG**",
        "\t\t- *2026-11-20* — started",
        "\t- FUTURE WORK",
        "\t\t- later",
        "\t- a loose note after the drawer",
    ]);
    let pass = tidied(&contents);
    assert_eq!(
        pass.updated.as_deref(),
        Some(
            note(&[
                "- [ ] #task Plain task",
                "\t- FUTURE WORK",
                "\t\t- later",
                "\t- a loose note after the drawer",
                "\t- 🛠️ **WORK LOG**",
                "\t\t- *2026-11-20* — started",
            ])
            .as_str()
        )
    );
    assert_eq!(pass.repairs[0].notes_filed, 0);
    assert!(!pass.repairs[0].notes_created);
}

#[test]
fn every_open_task_in_a_note_is_repaired_with_shifted_lines() {
    let contents = note(&[
        "- [ ] #task First",
        "\t- first note",
        "\t- [ ] first req",
        "- [ ] #task Canonical",
        "\t- fine",
        "- [ ] #task Second",
        "\t- second note",
        "\t- [ ] second req",
    ]);
    let pass = tidied(&contents);
    assert_eq!(
        pass.updated.as_deref(),
        Some(
            note(&[
                "- [ ] #task First",
                "\t- [ ] first req",
                "\t- NOTES",
                "\t\t- first note",
                "- [ ] #task Canonical",
                "\t- fine",
                "- [ ] #task Second",
                "\t- [ ] second req",
                "\t- NOTES",
                "\t\t- second note",
            ])
            .as_str()
        )
    );
    let lines: Vec<usize> = pass
        .repairs
        .iter()
        .map(|repair| repair.line_index)
        .collect();
    assert_eq!(lines, [0, 6], "lines are where the tasks sit after tidy");
    assert_eq!(pass.open_tasks, 3);
}

#[test]
fn a_nested_subtask_is_repaired_after_its_parent_moves_it() {
    let contents = note(&[
        "- [ ] #task Parent",
        "\t- 🛠️ **WORK LOG**",
        "\t\t- *2026-11-20* — started",
        "\t- [ ] #task Child",
        "\t\t- child note",
        "\t\t- [ ] child req",
    ]);
    let pass = tidied(&contents);
    assert_eq!(
        pass.updated.as_deref(),
        Some(
            note(&[
                "- [ ] #task Parent",
                "\t- [ ] #task Child",
                "\t\t- [ ] child req",
                "\t\t- NOTES",
                "\t\t\t- child note",
                "\t- 🛠️ **WORK LOG**",
                "\t\t- *2026-11-20* — started",
            ])
            .as_str()
        )
    );
    let repaired: Vec<(&str, usize)> = pass
        .repairs
        .iter()
        .map(|repair| (repair.text.as_str(), repair.line_index))
        .collect();
    assert_eq!(repaired, [("Parent", 0), ("Child", 1)]);
}

#[test]
fn a_req_owning_prj_files_loose_notes_under_the_notes_section() {
    let contents = note(&[
        "---",
        "type: \"[[project]]\"",
        "---",
        "- [ ] #task #prj Pk root ^prj",
        "\t- [ ] hand req",
        "\t- loose",
        "\t\t- nested",
        "",
        "## Tasks",
        "- [ ] #task Row a",
    ]);
    let pass = tidied(&contents);
    assert_eq!(
        pass.updated.as_deref(),
        Some(
            note(&[
                "---",
                "type: \"[[project]]\"",
                "---",
                "- [ ] #task #prj Pk root ^prj",
                "\t- [ ] hand req",
                "",
                "## Tasks",
                "- [ ] #task Row a",
                "",
                "## Notes",
                "",
                "- loose",
                "\t- nested",
            ])
            .as_str()
        )
    );
    assert_eq!(pass.repairs.len(), 1);
    assert_eq!(pass.repairs[0].notes_filed, 1);
    assert!(pass.repairs[0].notes_created);
}

#[test]
fn crlf_notes_keep_their_endings() {
    let contents = "- [ ] #task Crlf\r\n\t- a note\r\n\t- [ ] a req\r\n";
    let pass = tidied(contents);
    assert_eq!(
        pass.updated.as_deref(),
        Some("- [ ] #task Crlf\r\n\t- [ ] a req\r\n\t- NOTES\r\n\t\t- a note\r\n")
    );
}

#[test]
fn content_check_refuses_dropped_or_invented_lines() {
    let before = note(&["- [ ] #task T", "\t- a", "\t- b"]);
    assert!(content_preserved(&before, &before, 0).is_ok());
    let dropped = note(&["- [ ] #task T", "\t- a"]);
    assert!(content_preserved(&before, &dropped, 0)
        .unwrap_err()
        .contains("drop"));
    let invented = note(&["- [ ] #task T", "\t- a", "\t- b", "\t- c"]);
    assert!(content_preserved(&before, &invented, 0)
        .unwrap_err()
        .contains("add"));
    let with_notes =
        note(&["- [ ] #task T", "\t- NOTES", "\t\t- a", "\t\t- b"]);
    assert!(content_preserved(&before, &with_notes, 1).is_ok());
    assert!(content_preserved(&before, &with_notes, 0).is_err());
    let with_section = note(&[
        "- [ ] #task #prj Pk root ^prj",
        "\t- [ ] a req",
        "",
        "## Notes",
        "",
        "- a",
        "- b",
    ]);
    let before_prj = note(&[
        "- [ ] #task #prj Pk root ^prj",
        "\t- [ ] a req",
        "\t- a",
        "\t- b",
    ]);
    assert!(content_preserved(&before_prj, &with_section, 1).is_ok());
    assert!(content_preserved(&before_prj, &with_section, 0).is_err());
}

#[test]
fn fenced_lookalikes_are_not_tasks() {
    let contents = note(&[
        "```",
        "- [ ] #task Example",
        "\t- note",
        "\t- [ ] req",
        "```",
    ]);
    let pass = tidied(&contents);
    assert_eq!(pass.open_tasks, 0);
    assert_eq!(pass.updated, None);
}

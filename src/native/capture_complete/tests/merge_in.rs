use super::{
    super::model::{Candidates, PickerScope},
    task_links::{task_link_fixture, task_link_result},
    write_file, TempDir,
};
use crate::native::capture_language::CompletionContext;

fn merge_in_fixture() -> (TempDir, std::path::PathBuf) {
    let temp = TempDir::new("bob-cli-capture-complete-merge-in");
    let day_file = task_link_fixture(temp.path());
    write_file(
        &temp.path().join("health.md"),
        concat!(
            "---\n",
            "type: [[area]]\n",
            "project_name_aliases: [\"clinic\"]\n",
            "---\n",
            "- [ ] #task Book dentist ^dentist\n",
            "  - Call clinic\n",
            "- [ ] #task Loose healthy task\n",
            "- [ ] #task Has open child ^parent\n",
            "  - [ ] #task Child task ^child\n",
            "- [ ] #task Project root ^prj\n",
        ),
    );
    (temp, day_file)
}

#[test]
fn merge_in_note_and_vault_pickers_keep_only_mergeable_rows() {
    let (temp, day_file) = merge_in_fixture();

    let raw = "New task < @health+";
    let note = task_link_result(temp.path(), &day_file, raw, raw.len());
    assert_eq!(note.context, Some(CompletionContext::Task));
    let Candidates::Task(note_rows) = &note.candidates else {
        panic!("expected note-scoped task candidates");
    };
    let loose = note_rows
        .iter()
        .find(|candidate| candidate.text == "Loose healthy task")
        .expect("ID-less mergeable task remains selectable");
    assert!(loose.requires_block_id);
    assert_eq!(loose.detail.as_deref(), Some("⤵ merge in · title only"));
    let dentist = note_rows
        .iter()
        .find(|candidate| candidate.text == "Book dentist")
        .expect("task with one authored note");
    assert_eq!(dentist.detail.as_deref(), Some("⤵ merge in · 1 note"));
    assert!(note_rows.iter().all(|candidate| {
        candidate.text != "Has open child" && candidate.text != "Project root"
    }));

    let raw = "New task < +";
    let vault = task_link_result(temp.path(), &day_file, raw, raw.len());
    assert_eq!(vault.context, Some(CompletionContext::TaskParent));
    assert_eq!(
        vault.picker.as_ref().map(|picker| picker.scope),
        Some(PickerScope::Vault)
    );
    assert_eq!(
        vault
            .picker
            .as_ref()
            .and_then(|picker| picker.continuation.as_deref()),
        Some(" +")
    );
    let Candidates::TaskParent(vault_rows) = &vault.candidates else {
        panic!("expected vault-wide task-parent candidates");
    };
    assert!(vault_rows.iter().any(|candidate| {
        candidate.text == "Loose healthy task"
            && candidate.requires_block_id
            && candidate.detail.as_deref() == Some("⤵ merge in · title only")
    }));
    assert!(vault_rows.iter().all(|candidate| {
        candidate.text != "Has open child" && candidate.text != "Project root"
    }));
}

#[test]
fn merge_in_excludes_typed_sources_through_project_aliases() {
    let (temp, day_file) = merge_in_fixture();
    let raw = "New task < @clinic+dentist +";
    let result = task_link_result(temp.path(), &day_file, raw, raw.len());
    assert_eq!(result.context, Some(CompletionContext::TaskParent));
    let Candidates::TaskParent(rows) = result.candidates else {
        panic!("expected vault-wide task-parent candidates");
    };
    assert!(
        rows.iter()
            .all(|candidate| candidate.text != "Book dentist"),
        "the aliased typed source must be excluded: {rows:?}"
    );
}

#[test]
fn transfer_source_picker_keeps_idless_rows_for_add_block_id() {
    let (temp, day_file) = merge_in_fixture();
    let raw = "@health+ > @mac_inbox";
    let cursor = "@health+".len();
    let result = task_link_result(temp.path(), &day_file, raw, cursor);
    assert_eq!(result.context, Some(CompletionContext::Task));
    let Candidates::Task(rows) = result.candidates else {
        panic!("expected transfer source task candidates");
    };
    assert!(rows.iter().any(|candidate| {
        candidate.text == "Loose healthy task" && candidate.requires_block_id
    }));
}

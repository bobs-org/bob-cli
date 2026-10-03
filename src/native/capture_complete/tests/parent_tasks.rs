use super::{
    super::{
        model::{Candidates, PickerScope, Replacement},
        render::{candidate_lines, context_label},
        shell::shell_completion,
    },
    day_file_guard, result_all,
    task_links::task_link_fixture,
    with_env, write_file,
};
use crate::native::capture_language::CompletionContext;
use std::path::Path;

fn parent_result(
    root: &Path,
    day_file: &Path,
    raw: &str,
    cursor: usize,
    all_tasks: bool,
) -> super::super::model::CaptureCompleteResult {
    with_env("BOB_DAY_FILE", day_file, || {
        with_env("BOB_NOW", "2026-09-30 09:02:00", || {
            super::super::engine::build_result(root, raw, cursor, all_tasks)
                .expect("build parent-task completion")
        })
    })
}

#[test]
fn bare_plus_serves_vault_candidates_and_operator_hints() {
    let _guard = day_file_guard();
    let temp = super::TempDir::new("bob-cli-capture-complete-parent-task");
    let day_file = task_link_fixture(temp.path());

    let value = parent_result(temp.path(), &day_file, "+", 1, false);
    assert_eq!(value.context, Some(CompletionContext::TaskParent));
    assert_eq!(context_label(CompletionContext::TaskParent), "task_parent");
    assert_eq!(value.replacement, Replacement { start: 0, end: 1 });
    assert_eq!(value.query.as_deref(), Some(""));
    let picker = value.picker.as_ref().expect("picker descriptor");
    assert_eq!(picker.scope, PickerScope::Vault);
    assert_eq!(picker.scope_token, "+");
    assert_eq!(picker.note_target, None);
    assert_eq!(picker.marker_range, Replacement { start: 0, end: 1 });
    assert_eq!(picker.trigger_removal_range, picker.marker_range);
    let action_keys: Vec<&str> = picker
        .action_continuation_keys
        .as_ref()
        .expect("lone-plus action hints")
        .iter()
        .map(String::as_str)
        .collect();
    assert_eq!(
        action_keys,
        vec!["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "+"]
    );

    let Candidates::TaskParent(candidates) = &value.candidates else {
        panic!("expected task_parent candidates");
    };
    let replacements: Vec<&str> = candidates
        .iter()
        .map(|candidate| candidate.replacement.as_str())
        .collect();
    assert_eq!(
        replacements,
        vec![
            "@sase+deep-fix",
            "@sase+outline",
            "",
            "",
            "@bob+polish",
            "",
            "",
            "@sase+blog",
        ]
    );
    assert!(candidates.iter().all(|candidate| {
        candidate.requires_block_id || !candidate.replacement.is_empty()
    }));
    let json = serde_json::to_value(&value).expect("serialize completion");
    assert_eq!(json["context"], "task_parent");
    assert_eq!(json["picker"]["kind"], "parent_task");
    assert_eq!(json["picker"]["scope"], "vault");
    assert!(json["picker"]["note_target"].is_null());
    assert!(json["candidates"][0].get("pulls_forward").is_none());
    assert_eq!(
        json["candidates"][6]["block_id_suggestions"][0],
        "fix-flaky-gkeep"
    );

    let rows = candidate_lines(&value.candidates, value.context);
    assert_eq!(rows[0].0, "@sase+deep-fix");
    assert!(rows[0].1.contains("Fix deep bug"));
    assert!(rows[0].1.contains("BUGS"));
    assert!(rows[6].0.starts_with("@sase+"));

    // The literal lone plus remains available as a shell marker too, but
    // shell extraction only emits identified rows and never assigns IDs.
    let shell = with_env("BOB_DAY_FILE", &day_file, || {
        with_env("BOB_NOW", "2026-09-30 09:02:00", || {
            shell_completion(temp.path(), "+", 1).expect("shell result")
        })
    })
    .expect("parent plus shell completion");
    assert_eq!(shell.marker_start, 0);
    assert!(shell.rows.iter().all(|row| !row.full.is_empty()));
    assert!(shell.rows.iter().all(|row| !row.full.ends_with("+")));
    assert!(shell.rows.iter().any(|row| row.full == "@sase+deep-fix"));
}

#[test]
fn plus_query_ranks_candidates_and_scoped_descriptors_keep_exact_ranges() {
    let _guard = day_file_guard();
    let temp =
        super::TempDir::new("bob-cli-capture-complete-parent-task-query");
    let day_file = task_link_fixture(temp.path());
    write_file(
        &temp.path().join("cash.md"),
        "---\ntype: [[area]]\n---\n- [ ] #task Buy oat milk ^buy-milk\n- [ ] #task No ID yet\n",
    );

    let raw = "Called the bank +bank";
    let value = parent_result(temp.path(), &day_file, raw, raw.len(), false);
    assert_eq!(value.context, Some(CompletionContext::TaskParent));
    assert_eq!(value.query.as_deref(), Some("bank"));
    assert_eq!(value.replacement, Replacement { start: 16, end: 21 });
    let Candidates::TaskParent(candidates) = &value.candidates else {
        panic!("expected task_parent candidates");
    };
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].replacement, "");
    assert!(candidates[0].requires_block_id);

    // At replacement.start the query is empty, allowing the Mac client to
    // refetch the complete local snapshot after opening the picker.
    let full = parent_result(temp.path(), &day_file, raw, 16, false);
    assert_eq!(full.query.as_deref(), Some(""));
    assert_eq!(full.replacement, value.replacement);

    let scoped = result_all(temp.path(), "@Cash+", 6);
    assert_eq!(scoped.context, Some(CompletionContext::Task));
    assert_eq!(scoped.query.as_deref(), Some(""));
    let picker = scoped.picker.as_ref().expect("scoped picker");
    assert_eq!(picker.scope, PickerScope::Note);
    assert_eq!(picker.scope_token, "@cash+");
    assert_eq!(picker.note_target.as_deref(), Some("cash.md"));
    assert_eq!(picker.marker_range, Replacement { start: 0, end: 6 });
    assert_eq!(
        picker.trigger_removal_range,
        Replacement { start: 5, end: 6 }
    );
    let Candidates::Task(tasks) = &scoped.candidates else {
        panic!("expected scoped task candidates");
    };
    let idless = tasks
        .iter()
        .find(|task| task.requires_block_id)
        .expect("all-tasks ID-less row");
    assert!(idless
        .block_id_suggestions
        .as_ref()
        .is_some_and(|suggestions| !suggestions.is_empty()));

    let global = result_all(temp.path(), "@@Cash+", 7);
    assert_eq!(global.context, Some(CompletionContext::Task));
    let picker = global.picker.as_ref().expect("global scoped picker");
    assert_eq!(picker.scope_token, "@@cash+");
    assert_eq!(picker.note_target.as_deref(), Some("cash.md"));
    assert_eq!(picker.marker_range, Replacement { start: 0, end: 7 });
    assert_eq!(
        picker.trigger_removal_range,
        Replacement { start: 6, end: 7 }
    );
}

#[test]
fn vault_catalog_excludes_non_capture_and_terminal_notes() {
    let _guard = day_file_guard();
    let temp = super::TempDir::new("bob-cli-capture-complete-parent-catalog");
    let day_file = task_link_fixture(temp.path());

    let value = parent_result(temp.path(), &day_file, "+", 1, false);
    let Candidates::TaskParent(candidates) = &value.candidates else {
        panic!("expected task_parent candidates");
    };
    let routes: Vec<&str> = candidates
        .iter()
        .map(|candidate| candidate.route.as_str())
        .collect();
    assert!(
        !routes
            .iter()
            .any(|route| *route == "archive" || *route == "scratch"),
        "vault-wide plus must not advertise notes outside the capture catalog: {routes:?}"
    );
    assert!(!candidates.iter().any(|candidate| {
        candidate.text.contains("Leftover")
            || candidate.text.contains("Loose end")
    }));
}

#[test]
fn leading_plus_query_refetch_keeps_token_range_and_full_catalog() {
    let _guard = day_file_guard();
    let temp = super::TempDir::new("bob-cli-capture-complete-parent-refetch");
    let day_file = task_link_fixture(temp.path());

    let filtered = parent_result(temp.path(), &day_file, "+bank", 5, false);
    assert_eq!(filtered.context, Some(CompletionContext::TaskParent));
    assert_eq!(filtered.query.as_deref(), Some("bank"));
    assert_eq!(filtered.replacement, Replacement { start: 0, end: 5 });
    let Candidates::TaskParent(filtered_rows) = &filtered.candidates else {
        panic!("expected filtered task_parent candidates");
    };
    assert_eq!(filtered_rows.len(), 1);
    assert_eq!(filtered_rows[0].text, "Call the bank");

    let full = parent_result(temp.path(), &day_file, "+bank", 0, false);
    assert_eq!(full.query.as_deref(), Some(""));
    assert_eq!(full.replacement, filtered.replacement);
    assert_eq!(
        full.picker.as_ref().unwrap().marker_range,
        filtered.replacement
    );
    let Candidates::TaskParent(full_rows) = &full.candidates else {
        panic!("expected full task_parent snapshot");
    };
    assert!(
        full_rows.len() > filtered_rows.len(),
        "refetch at replacement.start must restore the unfiltered catalog"
    );
}

#[test]
fn scoped_missing_and_empty_notes_keep_picker_and_empty_catalog() {
    let _guard = day_file_guard();
    let temp = super::TempDir::new("bob-cli-capture-complete-parent-empty");
    let _day_file = task_link_fixture(temp.path());
    write_file(&temp.path().join("empty.md"), "---\ntype: [[area]]\n---\n");

    let missing = result_all(temp.path(), "@ghost+", 7);
    assert_eq!(missing.context, Some(CompletionContext::Task));
    let picker = missing.picker.as_ref().expect("missing-note picker");
    assert_eq!(picker.scope, PickerScope::Note);
    assert_eq!(picker.note_target.as_deref(), Some("ghost.md"));
    let Candidates::Task(missing_rows) = &missing.candidates else {
        panic!("expected scoped task candidates");
    };
    assert!(missing_rows.is_empty(), "{missing_rows:?}");

    let empty = result_all(temp.path(), "@empty+", 7);
    assert_eq!(empty.context, Some(CompletionContext::Task));
    assert_eq!(
        empty.picker.as_ref().unwrap().note_target.as_deref(),
        Some("empty.md")
    );
    let Candidates::Task(empty_rows) = &empty.candidates else {
        panic!("expected scoped task candidates");
    };
    assert!(empty_rows.is_empty(), "{empty_rows:?}");
}

#[test]
fn unicode_duplicates_and_queued_pomodoros_stay_in_catalog() {
    let _guard = day_file_guard();
    let temp = super::TempDir::new("bob-cli-capture-complete-parent-unicode");
    let day_file = task_link_fixture(temp.path());
    write_file(
        &temp.path().join("cash.md"),
        "---\ntype: [[area]]\n---\n\
         - [*] #task Finish Google Exit Packet! ^goog-exit\n\
         - [ ] #task Review notes ^cash-review\n\
         - [ ] #task Café 日本語 ^cafe\n\
         - [ ] #task A very long task that should stay searchable in the plus picker even when the title wraps past the panel width\n",
    );
    write_file(
        &temp.path().join("sase.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n\
         - [*] #task Review notes ^sase-review\n\
         - [*] #task Fix deep bug ^deep-fix\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n\
         - [ ] () — BUGS\n\t- [[sase#^deep-fix]]\n\
         - [ ] () — ADMIN\n\t- [[cash#^goog-exit]]\n",
    );

    let vault = parent_result(temp.path(), &day_file, "+", 1, false);
    let Candidates::TaskParent(rows) = &vault.candidates else {
        panic!("expected task_parent candidates");
    };
    let texts: Vec<&str> = rows.iter().map(|row| row.text.as_str()).collect();
    assert_eq!(
        texts.iter().filter(|text| **text == "Review notes").count(),
        2,
        "{texts:?}"
    );
    assert!(texts.iter().any(|text| text.contains("Café 日本語")));
    assert!(texts
        .iter()
        .any(|text| text.starts_with("A very long task")));
    let queued: Vec<&str> = rows
        .iter()
        .filter(|row| {
            row.group
                == crate::native::capture_link_tasks::LinkTaskGroup::Queued
        })
        .map(|row| row.replacement.as_str())
        .collect();
    assert!(queued.contains(&"@sase+deep-fix"), "{queued:?}");
    assert!(queued.contains(&"@cash+goog-exit"), "{queued:?}");

    let cafe = parent_result(
        temp.path(),
        &day_file,
        "+日本語",
        "+日本語".len(),
        false,
    );
    let Candidates::TaskParent(cafe_rows) = &cafe.candidates else {
        panic!("expected unicode query matches");
    };
    assert_eq!(cafe_rows.len(), 1);
    assert_eq!(cafe_rows[0].replacement, "@cash+cafe");
}

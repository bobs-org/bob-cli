use std::path::{Path, PathBuf};

use super::{
    super::{
        model::{Candidates, Replacement},
        render::candidate_lines,
    },
    result, write_file, write_settings, TempDir,
};
use crate::native::capture_language::CompletionContext;
pub(super) fn active_task_fixture(root: &Path) -> PathBuf {
    write_settings(root);
    write_file(
        &root.join("sase.md"),
        concat!(
            "- [/] #task Outline talk ^outline\n",
            "- [*] #task Fix deep bug ^deep-fix\n",
            "- [ ] #task Ready thing ^ready\n",
        ),
    );
    let day_file = root.join("2026/20260710.md");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () — BUGS\n  - [[sase#^deep-fix]]\n",
    );
    day_file
}

#[test]
fn active_task_completion_offers_queued_tasks_first() {
    let temp = TempDir::new("bob-cli-capture-complete-active-task");
    let day_file = active_task_fixture(temp.path());

    let value = crate::native::env::with_var("BOB_DAY_FILE", &day_file, || {
        result(temp.path(), "^", 1)
    });
    assert_eq!(value.context, Some(CompletionContext::ActiveTask));
    assert_eq!(value.replacement, Replacement { start: 1, end: 1 });
    let Candidates::ActiveTask(candidates) = &value.candidates else {
        panic!("expected active-task candidates");
    };
    // Ready tasks are excluded; the queued Next task sorts first.
    let replacements: Vec<&str> = candidates
        .iter()
        .map(|candidate| candidate.replacement.as_str())
        .collect();
    assert_eq!(replacements, vec!["sase:deep-fix", "sase:outline"]);

    let queued = &candidates[0];
    assert_eq!(queued.route, "sase");
    assert_eq!(queued.block_id, "deep-fix");
    assert_eq!(queued.status_symbol, '*');
    assert_eq!(queued.status_type, "ON_HOLD");
    assert_eq!(queued.text, "Fix deep bug");
    let pomodoro = queued.pomodoro.as_ref().expect("queued task");
    assert_eq!(pomodoro.name.as_deref(), Some("BUGS"));
    assert!(!pomodoro.is_current);

    let unqueued = &candidates[1];
    assert_eq!(unqueued.status_symbol, '/');
    assert!(unqueued.pomodoro.is_none());
    assert!(value.warnings.is_empty());
}

#[test]
fn active_task_completion_ranks_queries_and_pins_json_shape() {
    let temp = TempDir::new("bob-cli-capture-complete-active-rank");
    let day_file = active_task_fixture(temp.path());

    let raw = "^sase:dee";
    let value = crate::native::env::with_var("BOB_DAY_FILE", &day_file, || {
        result(temp.path(), raw, raw.len())
    });
    assert_eq!(value.context, Some(CompletionContext::ActiveTask));
    assert_eq!(value.replacement, Replacement { start: 1, end: 9 });
    let Candidates::ActiveTask(candidates) = &value.candidates else {
        panic!("expected active-task candidates");
    };
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].replacement, "sase:deep-fix");

    let json = serde_json::to_value(&value).expect("serialize result");
    assert_eq!(json["context"], "active_task");
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": 1, "end": 9})
    );
    let candidate = &json["candidates"][0];
    assert_eq!(candidate["replacement"], "sase:deep-fix");
    assert_eq!(candidate["ref"], candidates[0].task_ref);
    assert_eq!(candidate["route"], "sase");
    assert_eq!(candidate["block_id"], "deep-fix");
    assert_eq!(candidate["status_symbol"], "*");
    assert_eq!(candidate["text"], "Fix deep bug");
    assert_eq!(candidate["pomodoro"]["name"], "BUGS");
    assert_eq!(candidate["pomodoro"]["is_current"], false);
}

#[test]
fn active_task_completion_excludes_ready_tasks() {
    let temp = TempDir::new("bob-cli-capture-complete-active-now");
    write_settings(temp.path());
    write_file(
        &temp.path().join("sase.md"),
        concat!(
            "- [*] #task Fix deep bug ^deep-fix\n",
            "- [ ] #task Ready bet #now ^ready-now\n",
            "- [ ] #task Ready thing ^ready\n",
        ),
    );
    let day_file = temp.path().join("2026/20260710.md");
    write_file(&day_file, "## Pomodoros\n- [ ] () — BUGS\n");

    let value = crate::native::env::with_var("BOB_DAY_FILE", &day_file, || {
        result(temp.path(), "^", 1)
    });
    assert_eq!(value.context, Some(CompletionContext::ActiveTask));
    let Candidates::ActiveTask(candidates) = &value.candidates else {
        panic!("expected active-task candidates");
    };
    // Ready tasks stay excluded even with `#now` text; only the
    // Next task lists, with no `now` key in its JSON.
    let replacements: Vec<&str> = candidates
        .iter()
        .map(|candidate| candidate.replacement.as_str())
        .collect();
    assert_eq!(replacements, vec!["sase:deep-fix"]);

    let json = serde_json::to_value(&value).expect("serialize result");
    assert!(json["candidates"][0].get("now").is_none());
}

#[test]
fn active_task_completion_keeps_suffixes_and_names_pomodoros() {
    let temp = TempDir::new("bob-cli-capture-complete-active-suffix");
    let day_file = active_task_fixture(temp.path());

    // The replacement always stops before `#`/`=`.
    let raw = "^sase:deep-fix#bu";
    let link = crate::native::env::with_var("BOB_DAY_FILE", &day_file, || {
        result(temp.path(), raw, 14)
    });
    assert_eq!(link.context, Some(CompletionContext::ActiveTask));
    assert_eq!(link.replacement, Replacement { start: 1, end: 14 });

    // After `#` the same marker completes Pomodoro names.
    let name = crate::native::env::with_var("BOB_DAY_FILE", &day_file, || {
        result(temp.path(), raw, raw.len())
    });
    assert_eq!(name.context, Some(CompletionContext::PomodoroName));
    let Candidates::PomodoroName(names) = &name.candidates else {
        panic!("expected Pomodoro-name candidates");
    };
    assert_eq!(names[0].replacement, "bugs");

    // Inside `=<X>` there is no completion field at all.
    let raw = "^sase:deep-fix=3";
    let empty = crate::native::env::with_var("BOB_DAY_FILE", &day_file, || {
        result(temp.path(), raw, raw.len())
    });
    assert_eq!(empty.context, None);
    assert_eq!(empty.candidates.len(), 0);
}

#[test]
fn active_task_human_rows_name_the_queue() {
    let temp = TempDir::new("bob-cli-capture-complete-active-human");
    let day_file = active_task_fixture(temp.path());

    let value = crate::native::env::with_var("BOB_DAY_FILE", &day_file, || {
        result(temp.path(), "^", 1)
    });
    let rows = candidate_lines(&value.candidates, value.context);
    assert_eq!(
        rows,
        vec![
            (
                "sase:deep-fix".to_string(),
                "[*] Fix deep bug  · BUGS".to_string(),
            ),
            (
                "sase:outline".to_string(),
                "[/] Outline talk  · Not queued".to_string(),
            ),
        ]
    );
}

use super::{
    super::model::{Candidates, Replacement},
    day_file_guard, result, result_all, with_env, write_file, write_settings,
    TempDir,
};
use crate::native::{
    capture_block_ids, capture_language::CompletionContext,
    capture_targets::CaptureTargetKind,
};
#[test]
fn route_completion_ranks_prefix_matches_before_substring_matches() {
    let temp = TempDir::new("bob-cli-capture-complete-routes");
    write_file(&temp.path().join("cash.md"), "---\ntype: [[area]]\n---\n");
    write_file(
        &temp.path().join("cash-flow.md"),
        "---\ntype: [[area]]\n---\n",
    );
    write_file(
        &temp.path().join("petty-cash.md"),
        "---\ntype: [[area]]\n---\n",
    );

    let value = result(temp.path(), "@ca", 3);
    assert_eq!(value.context, Some(CompletionContext::Route));
    assert_eq!(value.replacement, Replacement { start: 1, end: 3 });
    let Candidates::Route(routes) = &value.candidates else {
        panic!("expected route candidates");
    };
    let names: Vec<&str> =
        routes.iter().map(|route| route.route.as_str()).collect();
    assert_eq!(names, vec!["cash", "cash-flow", "petty-cash"]);
}

#[test]
fn route_completion_lists_every_target_for_an_empty_query() {
    let temp = TempDir::new("bob-cli-capture-complete-routes-empty");
    let value = result(temp.path(), "@", 1);
    assert_eq!(value.context, Some(CompletionContext::Route));
    let Candidates::Route(routes) = &value.candidates else {
        panic!("expected route candidates");
    };
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].route, "mac_inbox");
    assert_eq!(routes[0].kind, CaptureTargetKind::Inbox);
}

#[test]
fn section_completion_lists_headings_of_the_resolved_route() {
    let temp = TempDir::new("bob-cli-capture-complete-sections");
    write_file(
        &temp.path().join("notes.md"),
        "# Ideas\n## Ignored\n## Tasks\n### Inbox Ideas\n",
    );

    let value = result(temp.path(), "Idea @notes#Id", 14);
    assert_eq!(value.context, Some(CompletionContext::Section));
    let Candidates::Section(sections) = &value.candidates else {
        panic!("expected section candidates");
    };
    let titles: Vec<&str> = sections
        .iter()
        .map(|section| section.title.as_str())
        .collect();
    assert_eq!(titles, vec!["Ideas", "Inbox Ideas"]);
}

#[test]
fn section_completion_on_a_missing_note_is_an_empty_success() {
    let temp = TempDir::new("bob-cli-capture-complete-sections-missing");
    let value = result(temp.path(), "Idea @notes#", 12);
    assert_eq!(value.context, Some(CompletionContext::Section));
    assert_eq!(value.candidates.len(), 0);
}

#[test]
fn pomodoro_block_id_completion_only_offers_tasks_with_a_block_id() {
    let temp = TempDir::new("bob-cli-capture-complete-pomodoro");
    write_settings(temp.path());
    write_file(
        &temp.path().join("dev.md"),
        concat!(
            "- [ ] #task No block ID\n",
            "- [ ] #task Focus session ^focus-123\n",
            "- [ ] #task Other focus ^focus-999\n",
        ),
    );

    // Marker-only `@route:` is link intent: identified linkable tasks.
    let value = result(temp.path(), "@Dev:foc", 8);
    assert_eq!(value.context, Some(CompletionContext::PomodoroBlockId));
    let Candidates::Task(tasks) = &value.candidates else {
        panic!("expected task candidates");
    };
    let ids: Vec<&str> = tasks
        .iter()
        .map(|task| task.block_id.as_deref().expect("identified"))
        .collect();
    assert_eq!(ids, vec!["focus-123", "focus-999"]);
    assert!(tasks.iter().all(|task| !task.requires_block_id));
    assert!(tasks.iter().all(|task| task.line > 0));
    let block_id = value.block_id.as_ref().expect("block_id object");
    assert_eq!(block_id.intent, capture_block_ids::BlockIdIntent::Link);

    // The same marker on an item with text is new intent: no candidates.
    let with_text = result(temp.path(), "Do work @Dev:foc", 16);
    assert_eq!(with_text.context, Some(CompletionContext::PomodoroBlockId));
    assert_eq!(with_text.candidates.len(), 0);
    let block_id = with_text.block_id.as_ref().expect("block_id object");
    assert_eq!(block_id.intent, capture_block_ids::BlockIdIntent::New);
}

#[test]
fn sub_bullet_task_completion_reports_full_task_metadata() {
    let temp = TempDir::new("bob-cli-capture-complete-sub-bullet");
    write_settings(temp.path());
    write_file(
        &temp.path().join("cash.md"),
        "# Tasks\n- [*] #task Finish Google Exit Packet! ^goog-exit\n",
    );

    let value = result(temp.path(), "note @Cash+goog", 15);
    assert_eq!(value.context, Some(CompletionContext::Task));
    let Candidates::Task(tasks) = &value.candidates else {
        panic!("expected task candidates");
    };
    assert_eq!(tasks.len(), 1);
    let task = &tasks[0];
    assert_eq!(task.replacement, "goog-exit");
    assert_eq!(task.block_id.as_deref(), Some("goog-exit"));
    assert_eq!(task.route, "cash");
    assert!(!task.requires_block_id);
    assert_eq!(task.text, "Finish Google Exit Packet!");
    assert_eq!(task.section.as_deref(), Some("Tasks"));
    assert_eq!(task.status_symbol, '*');
    assert_eq!(task.status_type, "ON_HOLD");
    assert_eq!(task.child_count, 0);
}

#[test]
fn task_section_completion_lists_ranked_slugs_for_the_parent_task() {
    let temp = TempDir::new("bob-cli-capture-complete-task-section");
    write_settings(temp.path());
    write_file(
        &temp.path().join("foo.md"),
        concat!(
            "# Tasks\n",
            "- [ ] #task Parent task ^bar\n",
            "\t- REQUIREMENTS\n",
            "\t\t- existing\n",
            "\t- FUTURE WORKFLOW\n",
            "\t- NOTES\n",
            "\t- FUTURE WORK\n",
        ),
    );

    let empty = result(temp.path(), "note @foo+bar#", 14);
    assert_eq!(empty.context, Some(CompletionContext::TaskSection));
    assert_eq!(empty.replacement, Replacement { start: 14, end: 14 });
    let Candidates::TaskSection(all) = &empty.candidates else {
        panic!("expected task section candidates");
    };
    let titles: Vec<&str> =
        all.iter().map(|section| section.title.as_str()).collect();
    assert_eq!(
        titles,
        ["REQUIREMENTS", "FUTURE WORKFLOW", "NOTES", "FUTURE WORK"]
    );
    assert_eq!(all[0].replacement, "requirements");
    assert_eq!(all[0].slug, "requirements");
    assert_eq!(all[0].route, "foo");
    assert_eq!(all[0].block_id.as_deref(), Some("bar"));
    assert_eq!(all[0].text, "Parent task");
    assert_eq!(all[0].line, 3);
    assert_eq!(all[0].child_count, 1);
    assert_eq!(all[3].replacement, "future-work");
    assert_eq!(all[3].child_count, 0);
    assert!(empty.warnings.is_empty());

    let prefix = result(temp.path(), "note @foo+bar#future", 20);
    let Candidates::TaskSection(prefixed) = &prefix.candidates else {
        panic!("expected task section candidates");
    };
    let prefixed_titles: Vec<&str> = prefixed
        .iter()
        .map(|section| section.title.as_str())
        .collect();
    assert_eq!(prefixed_titles, ["FUTURE WORKFLOW", "FUTURE WORK"]);

    let exact = result(temp.path(), "note @foo+bar#future-work", 25);
    let Candidates::TaskSection(exact_hits) = &exact.candidates else {
        panic!("expected task section candidates");
    };
    let exact_titles: Vec<&str> = exact_hits
        .iter()
        .map(|section| section.title.as_str())
        .collect();
    assert_eq!(exact_titles, ["FUTURE WORKFLOW", "FUTURE WORK"]);
    let future_work = exact_hits
        .iter()
        .find(|section| section.title == "FUTURE WORK")
        .expect("FUTURE WORK");
    assert_eq!(future_work.replacement, "future-work");
    assert_eq!(future_work.slug, "future-work");

    let substring = result(temp.path(), "note @foo+bar#work", 18);
    let Candidates::TaskSection(subs) = &substring.candidates else {
        panic!("expected task section candidates");
    };
    let sub_titles: Vec<&str> =
        subs.iter().map(|section| section.title.as_str()).collect();
    assert_eq!(sub_titles, ["FUTURE WORKFLOW", "FUTURE WORK"]);
}

#[test]
fn three_component_marker_keeps_route_and_task_contexts() {
    let temp = TempDir::new("bob-cli-capture-complete-three-component");
    write_settings(temp.path());
    write_file(
        &temp.path().join("foo.md"),
        concat!(
            "---\ntype: [[area]]\n---\n",
            "- [ ] #task Parent ^bar\n",
            "\t- REQUIREMENTS\n",
        ),
    );

    let raw = "note @foo+bar#req";
    let at = raw.find('@').expect("at");
    let plus = raw.find('+').expect("plus");
    let hash = raw.find('#').expect("hash");

    let route = result(temp.path(), raw, at + 3);
    assert_eq!(route.context, Some(CompletionContext::Route));
    let Candidates::Route(routes) = &route.candidates else {
        panic!("expected route candidates");
    };
    assert!(
        routes.iter().any(|candidate| candidate.route == "foo"),
        "{routes:?}"
    );

    let task = result(temp.path(), raw, plus + 2);
    assert_eq!(task.context, Some(CompletionContext::Task));
    let Candidates::Task(tasks) = &task.candidates else {
        panic!("expected task candidates");
    };
    assert_eq!(tasks[0].block_id.as_deref(), Some("bar"));

    let section = result(temp.path(), raw, hash + 2);
    assert_eq!(section.context, Some(CompletionContext::TaskSection));
    let Candidates::TaskSection(sections) = &section.candidates else {
        panic!("expected task section candidates");
    };
    assert_eq!(sections[0].replacement, "requirements");
    assert_eq!(sections[0].title, "REQUIREMENTS");
}

#[test]
fn task_section_completion_empty_block_id_is_an_empty_success() {
    let temp = TempDir::new("bob-cli-capture-complete-task-section-empty");
    write_settings(temp.path());
    write_file(
        &temp.path().join("foo.md"),
        "- [ ] #task Parent ^bar\n\t- REQUIREMENTS\n",
    );

    let value = result(temp.path(), "note @foo+#", 11);
    assert_eq!(value.context, Some(CompletionContext::TaskSection));
    assert_eq!(value.candidates.len(), 0);
    assert!(value.warnings.is_empty());
}

#[test]
fn task_section_completion_warns_once_for_an_unresolvable_parent() {
    let temp = TempDir::new("bob-cli-capture-complete-task-section-warning");
    write_settings(temp.path());
    write_file(
        &temp.path().join("foo.md"),
        concat!(
            "Plain heading ^plain-id\n",
            "- [ ] #task Ready ^ready-id\n",
            "- [ ] #task Dup ^dup-id\n",
            "- [ ] #task Also dup ^dup-id\n",
        ),
    );

    let missing = result(temp.path(), "note @foo+missing#", 18);
    assert_eq!(missing.context, Some(CompletionContext::TaskSection));
    assert_eq!(missing.candidates.len(), 0);
    assert_eq!(missing.warnings.len(), 1);
    assert_eq!(
        missing.warnings[0],
        "no task with block ID ^missing in foo.md"
    );
    assert!(!missing.warnings[0].contains("note @foo"));

    let close = result(temp.path(), "note @foo+ready-i#", 18);
    assert_eq!(close.warnings.len(), 1);
    assert!(
        close.warnings[0].contains("did you mean ^ready-id"),
        "{}",
        close.warnings[0]
    );

    let duplicate = result(temp.path(), "note @foo+dup-id#", 17);
    assert_eq!(duplicate.warnings.len(), 1);
    assert_eq!(
        duplicate.warnings[0],
        "block ID ^dup-id appears 2 times in foo.md"
    );

    let not_a_task = result(temp.path(), "note @foo+plain-id#", 19);
    assert_eq!(not_a_task.warnings.len(), 1);
    assert_eq!(not_a_task.warnings[0], "^plain-id in foo.md is not a task");
    assert!(!not_a_task.warnings[0].contains("Plain heading"));

    let missing_note = result(temp.path(), "note @absent+bar#", 17);
    assert_eq!(missing_note.warnings.len(), 1);
    assert_eq!(missing_note.warnings[0], "note does not exist: absent.md");
}

#[test]
fn hash_after_a_bare_block_id_marker_completes_a_pomodoro_name() {
    let _guard = day_file_guard();
    let temp = TempDir::new("bob-cli-capture-complete-task-toggle-hash");
    write_file(&temp.path().join("cash.md"), "- [ ] #task Parent ^bar\n");
    let day_file = temp.path().join("2026/20260828.md");
    write_file(&day_file, "## Pomodoros\n- [ ] () — BUGS\n- [ ] ()\n");

    let raw = "@cash+bar#bu";
    let value = with_env("BOB_DAY_FILE", &day_file, || {
        result(temp.path(), raw, raw.len())
    });

    assert_eq!(value.context, Some(CompletionContext::PomodoroName));
    let Candidates::PomodoroName(candidates) = &value.candidates else {
        panic!("expected Pomodoro-name candidates");
    };
    assert_eq!(candidates[0].replacement, "bugs");

    // The same marker with body text keeps the task-section context.
    let with_body = "note @cash+bar#bu";
    let section = result(temp.path(), with_body, with_body.len());
    assert_eq!(section.context, Some(CompletionContext::TaskSection));
}

#[test]
fn default_task_completion_stays_identified_only() {
    let temp = TempDir::new("bob-cli-capture-complete-identified-only");
    write_settings(temp.path());
    write_file(
        &temp.path().join("file.md"),
        concat!(
            "# Tasks\n",
            "- [ ] #task No block ID\n",
            "- [ ] #task Ready one ^ready-one\n",
            "- [x] #task Done task\n",
            "- [*] #task Ready two ^ready-two\n",
        ),
    );

    let value = result(temp.path(), "note @file+", 11);
    assert_eq!(value.context, Some(CompletionContext::Task));
    let Candidates::Task(tasks) = &value.candidates else {
        panic!("expected task candidates");
    };
    let ids: Vec<Option<&str>> =
        tasks.iter().map(|task| task.block_id.as_deref()).collect();
    assert_eq!(ids, vec![Some("ready-one"), Some("ready-two")]);
    assert!(tasks.iter().all(|task| !task.requires_block_id));
}

#[test]
fn all_tasks_lists_identified_tasks_before_unidentified_tasks() {
    let temp = TempDir::new("bob-cli-capture-complete-all-tasks");
    write_settings(temp.path());
    write_file(
        &temp.path().join("file.md"),
        concat!(
            "# Inbox\n",
            "- [ ] #task First missing\n",
            "- [ ] #task Ready one ^ready-one\n",
            "- [x] #task Done missing\n",
            "- [*] #task Ready two ^ready-two\n",
            "- [/] #task Second missing\n",
        ),
    );

    let value = result_all(temp.path(), "note @file+", 11);
    assert_eq!(value.context, Some(CompletionContext::Task));
    let Candidates::Task(tasks) = &value.candidates else {
        panic!("expected task candidates");
    };
    let rows: Vec<(Option<&str>, &str, bool)> = tasks
        .iter()
        .map(|task| {
            (
                task.block_id.as_deref(),
                task.text.as_str(),
                task.requires_block_id,
            )
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            (Some("ready-one"), "Ready one", false),
            (Some("ready-two"), "Ready two", false),
            (None, "First missing", true),
            (None, "Second missing", true),
        ]
    );
    assert_eq!(tasks[2].replacement, "");
    assert_eq!(tasks[2].route, "file");
    assert!(!tasks[2].task_ref.is_empty());
}

#[test]
fn all_tasks_search_keeps_identified_groups_ahead_of_unidentified() {
    let temp = TempDir::new("bob-cli-capture-complete-all-search");
    write_settings(temp.path());
    write_file(
        &temp.path().join("file.md"),
        concat!(
            "# Planning\n",
            "- [ ] #task Draft report\n",
            "- [ ] #task Ready alpha ^alpha-id\n",
            "# Review\n",
            "- [*] #task Planning notes ^later-id\n",
            "- [/] #task Alpha follow-up\n",
        ),
    );

    let by_id_and_text = result_all(temp.path(), "note @file+alpha", 16);
    let Candidates::Task(tasks) = &by_id_and_text.candidates else {
        panic!("expected task candidates");
    };
    let texts: Vec<&str> =
        tasks.iter().map(|task| task.text.as_str()).collect();
    assert_eq!(texts, vec!["Ready alpha", "Alpha follow-up"]);
    assert!(!tasks[0].requires_block_id);
    assert!(tasks[1].requires_block_id);

    let by_status = result_all(temp.path(), "note @file+Next", 15);
    let Candidates::Task(tasks) = &by_status.candidates else {
        panic!("expected task candidates");
    };
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].text, "Planning notes");
    assert_eq!(tasks[0].status_name, "Next");

    let by_section = result_all(temp.path(), "note @file+rev", 14);
    let Candidates::Task(tasks) = &by_section.candidates else {
        panic!("expected task candidates");
    };
    let texts: Vec<&str> =
        tasks.iter().map(|task| task.text.as_str()).collect();
    assert_eq!(texts, vec!["Planning notes", "Alpha follow-up"]);
}

#[test]
fn all_tasks_does_not_change_pomodoro_completion() {
    let temp = TempDir::new("bob-cli-capture-complete-all-pomodoro");
    write_settings(temp.path());
    write_file(
        &temp.path().join("dev.md"),
        concat!(
            "- [ ] #task No block ID\n",
            "- [ ] #task Focus session ^focus-123\n",
        ),
    );

    // Link intent stays identified-only even with `--all-tasks`.
    let value = result_all(temp.path(), "@Dev:foc", 8);
    assert_eq!(value.context, Some(CompletionContext::PomodoroBlockId));
    let Candidates::Task(tasks) = &value.candidates else {
        panic!("expected task candidates");
    };
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].block_id.as_deref(), Some("focus-123"));
    assert!(!tasks[0].requires_block_id);

    // New intent stays empty even with `--all-tasks`.
    let with_text = result_all(temp.path(), "Do work @Dev:foc", 16);
    assert_eq!(with_text.candidates.len(), 0);
}

#[test]
fn task_completion_before_an_explicit_toggle_bang_does_not_replace_the_bang() {
    let temp = TempDir::new("bob-cli-capture-complete-explicit-toggle");
    write_settings(temp.path());
    write_file(
        &temp.path().join("file.md"),
        "- [ ] #task Ready one ^ready-one\n",
    );
    let raw = "@file+ready!";
    let bang = raw.find('!').expect("bang");
    let value = result(temp.path(), raw, bang);
    assert_eq!(value.context, Some(CompletionContext::Task));
    assert_eq!(value.replacement.start, raw.find('+').expect("plus") + 1);
    assert_eq!(value.replacement.end, bang);
    let Candidates::Task(tasks) = &value.candidates else {
        panic!("expected task candidates");
    };
    assert_eq!(tasks[0].block_id.as_deref(), Some("ready-one"));
    assert_eq!(tasks[0].replacement, "ready-one");

    let after_bang = result(temp.path(), raw, raw.len());
    assert_eq!(after_bang.context, None);
}

#[test]
fn task_block_id_completion_offers_routes_but_not_authored_ids() {
    let temp = TempDir::new("bob-cli-capture-complete-task-block-id");
    write_file(&temp.path().join("cash.md"), "---\ntype: [[area]]\n---\n");
    write_file(
        &temp.path().join("dev.md"),
        "# Tasks\n- [ ] #task Existing ^existing-id\n",
    );

    let route_side = result(temp.path(), "Do @ca^new-id", 6);
    assert_eq!(route_side.context, Some(CompletionContext::Route));
    let Candidates::Route(routes) = &route_side.candidates else {
        panic!("expected route candidates");
    };
    assert_eq!(routes[0].route, "cash");

    // The right-hand side of `@route^` is now a `task_block_id`
    // completion with empty candidates and a `new`-intent block object.
    let id_side = result(temp.path(), "Do @dev^new-id", 14);
    assert_eq!(id_side.context, Some(CompletionContext::TaskBlockId));
    assert_eq!(id_side.candidates.len(), 0);
    let block_id = id_side.block_id.as_ref().expect("block_id object");
    assert_eq!(block_id.route, "dev");
    assert_eq!(block_id.marker, "^");
    assert_eq!(block_id.intent, capture_block_ids::BlockIdIntent::New);
    assert_eq!(block_id.allowed_character, "[A-Za-z0-9-]");
}

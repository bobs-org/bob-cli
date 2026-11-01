use super::{
    super::model::{Candidates, Replacement},
    result, write_file, write_settings, TempDir,
};
use crate::native::{capture_block_ids, capture_language::CompletionContext};
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
fn route_completion_matches_aliases_with_canonical_replacement_after_prefix() {
    let temp = TempDir::new("bob-cli-capture-complete-routes-alias");
    write_file(&temp.path().join("cash.md"), "---\ntype: [[area]]\n---\n");
    write_file(
        &temp.path().join("bob.md"),
        "---\ntype: [[project]]\nstatus: wip\nproject_name_aliases: [\"bob-cli\"]\n---\n",
    );
    // Canonical prefix matches first; the alias match carries the
    // canonical replacement with `match_kind: "alias"`.
    let value = result(temp.path(), "@bob-cli", 8);
    let Candidates::Route(routes) = &value.candidates else {
        panic!("expected route candidates");
    };
    let names: Vec<&str> =
        routes.iter().map(|route| route.route.as_str()).collect();
    assert_eq!(names, vec!["bob"]);
    assert_eq!(routes[0].replacement, "bob");
    assert_eq!(routes[0].match_kind, Some("alias"));
    // An empty query keeps existing behavior with no alias markers.
    let empty = result(temp.path(), "@", 1);
    let Candidates::Route(empty_routes) = &empty.candidates else {
        panic!("expected route candidates");
    };
    assert!(empty_routes.iter().all(|route| route.match_kind.is_none()));
    assert_eq!(empty_routes.len(), 3);
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
    assert_eq!(
        routes[0].kind,
        super::super::model::RouteCandidateKind::Inbox
    );
}

#[test]
fn double_at_offers_only_destinations_never_a_template_candidate() {
    let temp = TempDir::new("bob-cli-capture-complete-no-template-candidate");
    for (raw, cursor) in [("@@te", 4), ("@@", 2)] {
        let value = result(temp.path(), raw, cursor);
        let Candidates::Route(routes) = &value.candidates else {
            panic!("expected route candidates for {raw:?}");
        };
        assert!(
            routes.iter().all(|route| route.replacement != "template:"),
            "{raw:?}: {routes:?}"
        );
    }
}

#[test]
fn template_rows_and_placeholders_do_not_open_route_completion() {
    let temp = TempDir::new("bob-cli-capture-complete-template-gate");
    for (raw, cursor) in [
        ("Buy {0} @not-a-route %t\n\nrow", 27),
        ("Buy {0} @not-a-route %t\n\nrow", 6),
        ("Buy {0} @not-a-route %t\n\nrow  two", 29),
    ] {
        let value = result(temp.path(), raw, cursor);
        assert!(value.context.is_none(), "{raw:?} at {cursor}: {value:?}");
        assert!(value.candidates.is_empty());
    }

    // The existing `%t` token itself belongs to the directive picker. The
    // template gate must leave that context available while suppressing
    // route completion on template rows and placeholders.
    let directive = result(temp.path(), "Buy {0} @not-a-route %t\n\nrow", 22);
    assert_eq!(directive.context, Some(CompletionContext::Directive));
    let Candidates::Directive(rows) = directive.candidates else {
        panic!("expected directive candidates");
    };
    assert_eq!(
        rows.iter()
            .map(|row| row.token.as_str())
            .collect::<Vec<_>>(),
        vec!["%c", "%n", "%t"]
    );
}

#[test]
fn template_completion_still_completes_markers_in_the_definition() {
    let temp = TempDir::new("bob-cli-capture-complete-template-definition");
    let value = result(temp.path(), "Buy {0} @no %t\n\nrow", 11);
    assert_eq!(value.context, Some(CompletionContext::Route));
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

    let drawer = result(temp.path(), raw, hash + 2);
    assert_eq!(drawer.context, Some(CompletionContext::Drawer));
    let Candidates::Drawer(drawers) = &drawer.candidates else {
        panic!("expected drawer candidates");
    };
    assert_eq!(drawers[0].replacement, "#requirements");
    assert_eq!(drawers[0].title, "REQUIREMENTS");
}

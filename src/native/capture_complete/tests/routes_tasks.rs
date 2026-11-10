use super::{
    super::model::{Candidates, Replacement, SectionCandidate},
    result, write_file, write_settings, TempDir,
};
use crate::native::{capture_block_ids, capture_language::CompletionContext};

fn section_rows(candidates: &Candidates) -> Vec<&SectionCandidate> {
    let Candidates::Section(rows) = candidates else {
        panic!("expected section candidates");
    };
    rows.iter()
        .map(|row| row.as_section().expect("heading row"))
        .collect()
}
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
fn route_completion_offers_notebook_rows_after_projects_with_breadcrumb() {
    let temp = TempDir::new("bob-cli-capture-complete-routes-notebook");
    write_file(&temp.path().join("dev.md"), "---\ntype: [[area]]\n---\n");
    write_file(
        &temp.path().join("bob.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n",
    );
    write_file(
        &temp.path().join("dev_tools.md"),
        "---\nparent: \"[[dev]]\"\ntype: \"[[notebook]]\"\nsummary: Tools.\n---\n\n# **Developer** [[tools]]\n",
    );

    let value = result(temp.path(), "@", 1);
    let Candidates::Route(routes) = &value.candidates else {
        panic!("expected route candidates");
    };
    let names: Vec<&str> =
        routes.iter().map(|route| route.route.as_str()).collect();
    assert_eq!(names, vec!["mac_inbox", "dev", "bob", "dev_tools"]);
    let notebook = routes
        .iter()
        .find(|route| route.route == "dev_tools")
        .expect("notebook candidate");
    assert_eq!(
        notebook.kind,
        super::super::model::RouteCandidateKind::Notebook
    );
    assert_eq!(notebook.title.as_deref(), Some("Developer tools"));
    assert_eq!(notebook.summary.as_deref(), Some("Tools."));
    assert_eq!(notebook.owner.as_deref(), Some("dev"));
    assert_eq!(
        notebook.breadcrumb.as_deref(),
        Some(&["dev".to_string(), "Developer tools".to_string()][..])
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
    let sections = section_rows(&value.candidates);
    let titles: Vec<&str> = sections
        .iter()
        .map(|section| section.title.as_str())
        .collect();
    assert_eq!(titles, vec!["Ideas", "Inbox Ideas"]);
    assert_eq!(sections[0].replacement, "Ideas");
    assert_eq!(sections[1].replacement, "inbox-ideas");
}

#[test]
fn section_completion_round_trips_slug_and_quoted_replacements() {
    let temp = TempDir::new("bob-cli-capture-complete-section-roundtrip");
    write_file(
        &temp.path().join("sase.md"),
        concat!(
            "## Future Work\n",
            "future\n",
            "## future-work\n",
            "hyphen\n",
            "## Inbox Ideas\n",
            "inbox\n",
            "## A \"Q\"\n",
            "quoted\n",
            "## a-\"q\"\n",
            "hyphen-quote\n",
        ),
    );

    let value = result(temp.path(), "@sase#", 6);
    let sections = section_rows(&value.candidates);
    let by_title: std::collections::HashMap<&str, &str> = sections
        .iter()
        .map(|section| (section.title.as_str(), section.replacement.as_str()))
        .collect();
    assert_eq!(by_title["Future Work"], "\"Future Work\"");
    assert_eq!(by_title["future-work"], "future-work");
    assert_eq!(by_title["Inbox Ideas"], "inbox-ideas");
    assert_eq!(by_title["A \"Q\""], "\"A \\\"Q\\\"\"");

    let slug_query = result(temp.path(), "@sase#future-w", 14);
    let slug_rows = section_rows(&slug_query.candidates);
    let slug_titles: Vec<&str> =
        slug_rows.iter().map(|row| row.title.as_str()).collect();
    assert!(slug_titles.contains(&"Future Work"), "{slug_titles:?}");
    assert!(slug_titles.contains(&"future-work"), "{slug_titles:?}");
}

#[test]
fn section_completion_partial_quoted_query_covers_the_literal() {
    let temp = TempDir::new("bob-cli-capture-complete-section-quoted");
    write_file(&temp.path().join("sase.md"), "## Future Work\nnotes\n");

    let raw = "@sase#\"Fu";
    let value = result(temp.path(), raw, raw.len());
    assert_eq!(value.context, Some(CompletionContext::Section));
    assert_eq!(value.replacement.start, 6);
    assert_eq!(value.replacement.end, raw.len());
    let sections = section_rows(&value.candidates);
    assert_eq!(sections[0].title, "Future Work");
    assert_eq!(sections[0].replacement, "future-work");
}

#[test]
fn section_completion_on_a_list_header_with_bullets() {
    let temp = TempDir::new("bob-cli-capture-complete-section-header");
    write_file(&temp.path().join("sase.md"), "## Future Work\nnotes\n");

    let raw = "@sase#Fu\n- boom";
    let value = result(temp.path(), raw, "@sase#Fu".len());
    assert_eq!(value.context, Some(CompletionContext::Section));
    let sections = section_rows(&value.candidates);
    assert_eq!(sections[0].title, "Future Work");
    assert_eq!(sections[0].replacement, "future-work");
}

#[test]
fn section_completion_quote_leading_titles_use_exact_fallback() {
    let temp = TempDir::new("bob-cli-capture-complete-quote-leading");
    write_file(
        &temp.path().join("sase.md"),
        concat!(
            "PREAMBLE\n",
            "## \"Future Work\"\n",
            "TARGET\n",
            "## future-work\n",
            "DECOY\n",
            "## “Quoted”\n",
            "CURLY\n",
            "## quoted\n",
            "WORD\n",
        ),
    );

    let value = result(temp.path(), "@sase#", 6);
    let sections = section_rows(&value.candidates);
    let by_title: std::collections::HashMap<&str, &str> = sections
        .iter()
        .map(|section| (section.title.as_str(), section.replacement.as_str()))
        .collect();
    assert_eq!(by_title["\"Future Work\""], "\"\\\"Future Work\\\"\"");
    assert_eq!(by_title["future-work"], "future-work");
    assert_eq!(by_title["“Quoted”"], "\"“Quoted”\"");
    assert_eq!(by_title["quoted"], "quoted");

    let raw = "@sase#\"Fu\n- already";
    let partial = result(temp.path(), raw, "@sase#\"Fu".len());
    assert_eq!(partial.context, Some(CompletionContext::Section));
    assert_eq!(partial.replacement.start, 6);
    assert_eq!(partial.replacement.end, "@sase#\"Fu".len());
    let partial_rows = section_rows(&partial.candidates);
    let quoted = partial_rows
        .iter()
        .find(|row| row.title == "\"Future Work\"")
        .expect("quoted heading");
    assert_eq!(quoted.replacement, "\"\\\"Future Work\\\"\"");
}

#[test]
fn shell_completion_preserves_quoted_section_replacement() {
    let temp = TempDir::new("bob-cli-capture-complete-shell-quoted");
    write_file(
        &temp.path().join("sase.md"),
        "## Future Work\nnotes\n## future-work\nhyphen\n",
    );
    let shell = crate::native::capture_complete::shell::shell_completion(
        temp.path(),
        "@sase#",
        6,
    )
    .expect("shell result")
    .expect("section rows");
    let fulls: Vec<&str> =
        shell.rows.iter().map(|row| row.full.as_str()).collect();
    assert!(
        fulls.contains(&"@sase#\"Future Work\""),
        "shell protocol carries the raw quoted replacement: {fulls:?}"
    );
    assert!(
        !fulls.iter().any(|full| full.contains("@sase#\"\"")),
        "shell.rs must not wrap the quoted replacement again: {fulls:?}"
    );
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

fn write_notebook(temp: &TempDir, stem: &str, body: &str) {
    write_file(
        &temp.path().join(format!("{stem}.md")),
        &format!(
            "---\nparent: \"[[dev]]\"\ntype: \"[[notebook]]\"\nsummary: Notes.\n---\n\n# Title\n\n{body}"
        ),
    );
}

#[test]
fn notebook_sections_list_h2_only_in_document_order() {
    let temp = TempDir::new("bob-cli-capture-complete-notebook-sections");
    write_file(&temp.path().join("dev.md"), "---\ntype: [[area]]\n---\n");
    write_notebook(
        &temp,
        "dev_tools",
        "# Preamble H1\n\n- top note\n\n## Git\n\n- a\n\n### Deep\n\n- b\n\n## Merge Tools\n\n- c\n\n```\n## Fenced\n```\n",
    );
    let raw = "@dev_tools#";
    let value = result(temp.path(), raw, raw.len());
    assert_eq!(value.context, Some(CompletionContext::Section));
    let Candidates::Section(rows) = &value.candidates else {
        panic!("expected section candidates");
    };
    let titles: Vec<&str> = rows
        .iter()
        .filter_map(|row| {
            row.as_section().map(|section| section.title.as_str())
        })
        .collect();
    assert_eq!(titles, vec!["Git", "Merge Tools"], "{titles:?}");
    // Replacement round-trips: single-word title stays bare, multi-word
    // becomes its hyphen slug.
    let by_title: std::collections::HashMap<&str, &str> = rows
        .iter()
        .filter_map(|row| {
            row.as_section().map(|section| {
                (section.title.as_str(), section.replacement.as_str())
            })
        })
        .collect();
    assert_eq!(by_title["Git"], "Git");
    assert_eq!(by_title["Merge Tools"], "merge-tools");
    // Empty query offers no create row.
    assert!(
        rows.iter().all(|row| row.as_section().is_some()),
        "{rows:?}"
    );
}

#[test]
fn notebook_section_create_row_for_unrelated_slug_and_near_miss() {
    let temp = TempDir::new("bob-cli-capture-complete-notebook-create");
    write_file(&temp.path().join("dev.md"), "---\ntype: [[area]]\n---\n");
    write_notebook(&temp, "dev_tools", "## Git\n\n- a\n");
    // Exact title and slug suppress the create row.
    for raw in ["@dev_tools#Git", "@dev_tools#git"] {
        let value = result(temp.path(), raw, raw.len());
        let Candidates::Section(rows) = &value.candidates else {
            panic!("expected section candidates");
        };
        assert!(
            rows.iter().all(|row| row.as_section().is_some()),
            "{raw}: {rows:?}"
        );
    }
    // Unrelated slug appends a bare-slug create row.
    let raw = "@dev_tools#merge-tools";
    let value = result(temp.path(), raw, raw.len());
    let Candidates::Section(rows) = &value.candidates else {
        panic!("expected section candidates");
    };
    let create = rows
        .iter()
        .find_map(|row| match row {
            super::super::model::SectionMenuRow::NotebookCreate(create) => {
                Some(create)
            }
            _ => None,
        })
        .expect("create row");
    assert_eq!(create.replacement, "merge-tools");
    assert_eq!(create.title, "Merge Tools");
    assert_eq!(create.kind, "notebook_section_create");
    // Near-miss unquoted slug offers the quoted selector.
    let raw = "@dev_tools#gi";
    let value = result(temp.path(), raw, raw.len());
    let Candidates::Section(rows) = &value.candidates else {
        panic!("expected section candidates");
    };
    let create = rows
        .iter()
        .find_map(|row| match row {
            super::super::model::SectionMenuRow::NotebookCreate(create) => {
                Some(create)
            }
            _ => None,
        })
        .expect("near-miss create row");
    assert_eq!(create.replacement, "\"Gi\"");
    // Quoted title preserves the authored spelling.
    let raw = "@dev_tools#\"Merge tools\"";
    let value = result(temp.path(), raw, raw.len());
    let Candidates::Section(rows) = &value.candidates else {
        panic!("expected section candidates");
    };
    let create = rows
        .iter()
        .find_map(|row| match row {
            super::super::model::SectionMenuRow::NotebookCreate(create) => {
                Some(create)
            }
            _ => None,
        })
        .expect("quoted create row");
    assert_eq!(create.title, "Merge tools");
    // Applying the response replacement keeps exactly one `#` and the
    // advertised title is accepted by the shared resolver.
    let mut draft = raw.to_string();
    draft.replace_range(
        value.replacement.start..value.replacement.end,
        &create.replacement,
    );
    assert_eq!(draft.matches('#').count(), 1, "{draft:?}");
    let headings = crate::native::capture_language::notebook::notebook_sections(
        &std::fs::read_to_string(temp.path().join("dev_tools.md"))
            .expect("read"),
    );
    match crate::native::capture_language::notebook::resolve_notebook_section(
        &create.title,
        true,
        &headings,
    ) {
        crate::native::capture_language::notebook::NotebookSectionResolution::Create { title } => {
            assert_eq!(title, "Merge tools");
        }
        other => panic!("expected create, got {other:?}"),
    }
}

#[test]
fn notebook_block_ids_mark_notebook_kind_and_avoid_all_ids() {
    let temp = TempDir::new("bob-cli-capture-complete-notebook-blockids");
    write_file(&temp.path().join("dev.md"), "---\ntype: [[area]]\n---\n");
    write_notebook(
        &temp,
        "dev_tools",
        "## Git\n\n- Use merge ^merge-cmd\n\n- Plain bullet ^plain-id\n\n",
    );
    // `@nb^` stays new intent with the notebook marker.
    let raw = "Try mergiraf @dev_tools^";
    let value = result(temp.path(), raw, raw.len());
    assert_eq!(value.context, Some(CompletionContext::TaskBlockId));
    let block_id = value.block_id.as_ref().expect("block_id object");
    assert_eq!(block_id.intent, capture_block_ids::BlockIdIntent::New);
    assert_eq!(block_id.destination_kind, Some("notebook"));
    // Body derives from the note text without route syntax.
    assert_eq!(block_id.body, "Try mergiraf");
    // Used IDs include both task and non-task anchors; suggestions avoid
    // every actual ID in the file.
    let used: Vec<&str> = block_id
        .used
        .iter()
        .map(|entry| entry.id.as_str())
        .collect();
    assert!(used.contains(&"merge-cmd"), "{used:?}");
    assert!(used.contains(&"plain-id"), "{used:?}");
    for suggestion in &block_id.suggestions {
        assert!(!used.contains(&suggestion.as_str()), "{suggestion:?}");
    }
    // Ordinary task fields leave the new field absent.
    write_file(
        &temp.path().join("cash.md"),
        "---\ntype: [[area]]\n---\n- [ ] #task Work ^work\n",
    );
    let raw = "Do work @cash^";
    let value = result(temp.path(), raw, raw.len());
    let block_id = value.block_id.as_ref().expect("block_id object");
    assert_eq!(block_id.destination_kind, None);
    // `#section^` also carries the notebook kind.
    let raw = "Try mergiraf @dev_tools#Git^";
    let value = result(temp.path(), raw, raw.len());
    let block_id = value.block_id.as_ref().expect("block_id object");
    assert_eq!(block_id.intent, capture_block_ids::BlockIdIntent::New);
    assert_eq!(block_id.destination_kind, Some("notebook"));
    // Unicode body and cursors keep working.
    let raw = "caf\u{e9} notes @dev_tools^";
    let value = result(temp.path(), raw, raw.len());
    let block_id = value.block_id.as_ref().expect("block_id object");
    assert_eq!(block_id.destination_kind, Some("notebook"));
    assert!(block_id.body.contains("caf\u{e9}"), "{}", block_id.body);
}

#[test]
fn notebook_note_bullets_offer_idd_first_then_idless() {
    let temp = TempDir::new("bob-cli-capture-complete-notebook-notes");
    write_file(&temp.path().join("dev.md"), "---\ntype: [[area]]\n---\n");
    write_notebook(
        &temp,
        "dev_tools",
        "## Git\n\n- Use merge ^merge-cmd\n\n- Plain idea\n\n  - Nested detail\n\n- [ ] Task checkbox ^task-id\n\n# Not a section\n\n```\n- Fenced ^fenced-id\n```\n",
    );
    let raw = "note @dev_tools+";
    let value = result(temp.path(), raw, raw.len());
    assert_eq!(value.context, Some(CompletionContext::Task));
    let Candidates::Task(rows) = &value.candidates else {
        panic!("expected task candidates");
    };
    // Eligible: two top-level notes + one nested; checkbox, heading,
    // and fenced examples excluded.
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert!(
        rows.iter().all(|row| row.item_kind == Some("note")),
        "{rows:?}"
    );
    // ID'd first, then ID-less in document order.
    assert_eq!(rows[0].block_id.as_deref(), Some("merge-cmd"));
    assert_eq!(rows[0].replacement, "merge-cmd");
    assert!(!rows[0].requires_block_id);
    assert!(rows[1].requires_block_id);
    assert!(rows[2].requires_block_id);
    assert!(rows[1].line > rows[0].line);
    assert!(rows[2].line > rows[1].line);
    assert_eq!(rows[1].text, "Plain idea");
    assert_eq!(rows[2].text, "Nested detail");
    assert!(rows[1].block_id_suggestions.is_some());
    // Filtering by text and ID.
    let filtered = result(
        temp.path(),
        "note @dev_tools+merge",
        "note @dev_tools+merge".len(),
    );
    let Candidates::Task(rows) = &filtered.candidates else {
        panic!("expected task candidates");
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].block_id.as_deref(), Some("merge-cmd"));
    // Bare `+` stays task-only: a task note still offers tasks.
    write_file(
        &temp.path().join("cash.md"),
        "---\ntype: [[area]]\n---\n- [ ] #task Work ^work\n",
    );
    let value = result(temp.path(), "note @cash+", "note @cash+".len());
    let Candidates::Task(rows) = &value.candidates else {
        panic!("expected task candidates");
    };
    assert!(rows.iter().all(|row| row.item_kind.is_none()), "{rows:?}");
}

#[test]
fn notebook_note_duplicate_ids_are_disabled() {
    let temp = TempDir::new("bob-cli-capture-complete-notebook-dup");
    write_file(&temp.path().join("dev.md"), "---\ntype: [[area]]\n---\n");
    write_notebook(
        &temp,
        "dev_tools",
        "- First ^dup\n\n- Second ^dup\n\n- Plain\n",
    );
    let raw = "note @dev_tools+";
    let value = result(temp.path(), raw, raw.len());
    let Candidates::Task(rows) = &value.candidates else {
        panic!("expected task candidates");
    };
    let dup_rows: Vec<_> = rows
        .iter()
        .filter(|row| row.block_id.as_deref() == Some("dup"))
        .collect();
    assert_eq!(dup_rows.len(), 2);
    assert!(dup_rows.iter().all(|row| row.disabled_reason.is_some()));
    assert!(dup_rows.iter().all(|row| row.replacement.is_empty()));
}

#[test]
fn notebook_section_completion_keeps_block_id_suffix_range() {
    let temp = TempDir::new("bob-cli-capture-complete-notebook-suffix");
    write_file(&temp.path().join("dev.md"), "---\ntype: [[area]]\n---\n");
    write_notebook(&temp, "dev_tools", "## Git\n\n- a\n");
    let raw = "@dev_tools#Git^myid";
    let cursor = "@dev_tools#Git".len();
    let value = result(temp.path(), raw, cursor);
    assert_eq!(value.context, Some(CompletionContext::Section));
    assert_eq!(
        value.replacement,
        Replacement {
            start: "@dev_tools#".len(),
            end: "@dev_tools#Git".len()
        }
    );
    let mut draft = raw.to_string();
    let replacement = section_rows(&value.candidates)[0].replacement.clone();
    draft.replace_range(
        value.replacement.start..value.replacement.end,
        &replacement,
    );
    assert_eq!(draft.matches('#').count(), 1, "{draft:?}");
    assert!(draft.ends_with("^myid"), "{draft:?}");
}

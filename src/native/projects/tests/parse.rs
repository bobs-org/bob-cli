//! Parser and tag unit tests.
use super::*;

#[test]
fn task_tag_matches_tasks_plugin_boundaries() {
    assert!(contains_task_tag("#task"));
    assert!(contains_task_tag("prefix (#task)"));
    assert!(contains_task_tag("[#task]"));
    assert!(contains_task_tag("#task:"));
    assert!(contains_task_tag("#task,"));

    assert!(!contains_task_tag("prefix#task"));
    assert!(!contains_task_tag("#taskish"));
    assert!(!contains_task_tag("task #task/sub"));
}

#[test]
fn wikilink_target_extracts_normalized_note_names() {
    assert_eq!(wikilink_target("[[Bob]]").as_deref(), Some("bob"));
    assert_eq!(wikilink_target("\"[[Bob]]\"").as_deref(), Some("bob"));
    assert_eq!(wikilink_target("'[[Bob]]'").as_deref(), Some("bob"));
    assert_eq!(
        wikilink_target("[[projects/Bob|Alias]]").as_deref(),
        Some("bob")
    );
    assert_eq!(
        wikilink_target("[[projects/Bob#Heading]]").as_deref(),
        Some("bob")
    );
    assert_eq!(
        wikilink_target("[[projects/Bob#^block]]").as_deref(),
        Some("bob")
    );
    assert_eq!(
        wikilink_target("[[Projects/Bob#Heading|Alias]]").as_deref(),
        Some("bob")
    );

    assert_eq!(wikilink_target("Bob"), None);
    assert_eq!(wikilink_target("[[]]"), None);
    assert_eq!(wikilink_target("[[folder/]]"), None);
}

#[test]
fn project_parser_accepts_project_type_variants_and_counts_tasks() {
    let contents = r#"---
type: "[[project]]"
status: wip
---
- [ ] #task #prj Finish the project #hide ^prj
- [ ] #task shown one
- [/] #task shown in progress
- [*] #task shown next
- [ ] #task legacy priority is shown [p::1]
- [ ] #task hidden helper #hide
- [x] #task finished
- [-] #task canceled
- [ ] #taskish not a task
"#;
    let mut issues = Vec::new();
    let project = parse_project(Path::new("Alpha.md"), contents, &mut issues)
        .expect("project note");

    assert!(issues.is_empty());
    assert_eq!(project.status, ProjectStatus::Wip);
    assert_eq!(project.open_task_count, 6);
    assert_eq!(project.open_unhidden_count, 4);
    assert_eq!(project.prj_task.state, PrjTaskState::Open);
    assert!(project.prj_task.hidden);
    assert_eq!(project.prj_task.description, "Finish the project");
    assert_eq!(project.link_name, "alpha");
    assert_eq!(project.link_stem, "Alpha");
}

#[test]
fn project_parser_accepts_prj_tag_and_strips_it_from_description() {
    let mut issues = Vec::new();
    let tagged = parse_project(
        Path::new("Tagged.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task #prj Ship the outcome #hide ^prj\n",
        &mut issues,
    )
    .expect("project note");
    assert!(issues.is_empty());
    assert_eq!(tagged.prj_task.state, PrjTaskState::Open);
    assert!(tagged.prj_task.hidden);
    assert_eq!(tagged.prj_task.description, "Ship the outcome");

    // Legacy lines without #prj remain valid and parse identically.
    let legacy = parse_project(
        Path::new("Legacy.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task Ship the outcome #hide ^prj\n",
        &mut issues,
    )
    .expect("project note");
    assert!(issues.is_empty());
    assert_eq!(legacy.prj_task.state, PrjTaskState::Open);
    assert_eq!(legacy.prj_task.description, "Ship the outcome");
}

#[test]
fn project_parser_reads_parent_wikilink_target() {
    let project = parse_clean_project(
        "Projects/Child.md",
        "---\ntype: [[project]]\nparent: \"[[Areas/Parent#Now|Parent alias]]\"\n---\n- [ ] #task Ship #hide ^prj\n",
    );

    assert_eq!(project.name, "Projects/Child");
    assert_eq!(project.link_name, "child");
    assert_eq!(project.parent_target.as_deref(), Some("parent"));
}

#[test]
fn project_parser_accepts_bare_project_type_and_prj_states() {
    let mut issues = Vec::new();
    let done = parse_project(
        Path::new("Done.md"),
        "---\ntype: [[project]]\nstatus: done\n---\n- [X] #task Ship #hide ^prj\n",
        &mut issues,
    )
    .expect("bare project note");
    assert_eq!(done.status, ProjectStatus::Done);
    assert_eq!(done.prj_task.state, PrjTaskState::Done);

    let canceled = parse_project(
        Path::new("Canceled.md"),
        "---\ntype: [[project]]\nstatus: canceled\n---\n- [-] #task Stop #hide ^prj\n",
        &mut issues,
    )
    .expect("canceled project note");
    assert_eq!(canceled.prj_task.state, PrjTaskState::Canceled);
    assert!(issues.is_empty());
}

#[test]
fn project_parser_records_scheduled_and_placeholder_prj() {
    let contents = format!(
        "---\ntype: [[project]]\n---\n- [ ] #task {PLACEHOLDER_CRITERIA} #hide [scheduled::2026-06-11] ^prj\n"
    );
    let mut issues = Vec::new();
    let project =
        parse_project(Path::new("Placeholder.md"), &contents, &mut issues)
            .expect("project note");

    assert!(issues.is_empty());
    assert_eq!(project.prj_task.state, PrjTaskState::Open);
    assert_eq!(project.prj_task.scheduled.as_deref(), Some("2026-06-11"));
    assert!(project.prj_task.hidden);
    assert!(project.prj_task.placeholder);
    assert_eq!(project.prj_task.column(&Styler::plain()), "placeholder");
}

#[test]
fn project_parser_marks_unprioritized_prj_as_on_dash() {
    let mut issues = Vec::new();
    let project = parse_project(
        Path::new("OnDash.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task Ship ^prj\n",
        &mut issues,
    )
    .expect("project note");

    assert!(issues.is_empty());
    assert!(!project.prj_task.hidden);
    assert_eq!(project.prj_task.column(&Styler::plain()), "on dash");
}

#[test]
fn project_parser_reports_malformed_and_multiple_prj_lines() {
    let mut issues = Vec::new();
    let malformed = parse_project(
        Path::new("Malformed.md"),
        "---\ntype: [[project]]\n---\nComplete this ^prj\n",
        &mut issues,
    )
    .expect("project note");
    assert_eq!(malformed.prj_task.state, PrjTaskState::Malformed);
    assert!(issues[0].message.contains("malformed ^prj task"));

    issues.clear();
    let multiple = parse_project(
        Path::new("Multiple.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task One #hide ^prj\n- [ ] #task Two #hide ^prj\n",
        &mut issues,
    )
    .expect("project note");
    assert_eq!(multiple.prj_task.state, PrjTaskState::Multiple);
    assert!(issues[0].message.contains("multiple ^prj tasks"));
}

#[test]
fn non_project_notes_are_ignored() {
    let mut issues = Vec::new();
    let note = parse_project(
        Path::new("Note.md"),
        "---\ntype: [[ref]]\n---\n- [ ] #task ignored\n",
        &mut issues,
    );
    assert!(note.is_none());
    assert!(issues.is_empty());
}

#[test]
fn project_parser_records_prj_sub_block_marker_lines() {
    let project = parse_clean_project(
        "Parent.md",
        "---\ntype: [[project]]\n---\n- [ ] #task Ship parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[BareChild]] • [[Projects/AliasChild|Child alias]]\n\t- [[ManualChild]]\n\t- [[MentionedChild]] kickoff notes\n\t\t- 🧩 **Sub-projects:** [[Projects/DeepChild#Next]]\n\t- prose with [[InlineOnly]] link\n",
    );

    let lines = &project.prj_task.sub_block.lines;
    assert_eq!(lines.len(), 5);
    assert_eq!(project.open_task_count, 1);
    assert_eq!(project.open_unhidden_count, 0);
    assert_eq!(project.prj_task.sub_block.prj_indent, "");
    assert_eq!(lines[0].indentation, "\t");
    assert!(lines[0].is_marker);
    assert_eq!(
        lines[0].trimmed_text,
        "- 🧩 **Sub-projects:** [[BareChild]] • [[Projects/AliasChild|Child alias]]"
    );
    assert_eq!(
        lines[0]
            .links
            .iter()
            .map(|link| (link.link_name.as_str(), link.stem.as_str()))
            .collect::<Vec<_>>(),
        vec![("barechild", "BareChild"), ("aliaschild", "AliasChild"),]
    );
    assert_eq!(
        lines[1]
            .links
            .iter()
            .map(|link| link.link_name.as_str())
            .collect::<Vec<_>>(),
        vec!["manualchild"]
    );
    assert!(!lines[1].is_marker);
    assert!(!lines[2].is_marker);
    assert!(lines[3].is_marker);
    assert_eq!(lines[3].indentation, "\t\t");
    assert_eq!(lines[3].links[0].stem, "DeepChild");
    assert_eq!(lines[4].links[0].link_name, "inlineonly");
    assert!(!lines[4].is_marker);
}

#[test]
fn project_parser_stops_prj_sub_block_at_blank_line() {
    let project = parse_clean_project(
        "Parent.md",
        "---\ntype: [[project]]\n---\n- [ ] #task Ship parent #hide ^prj\n\n\t- [[NotInBlock]]\n",
    );

    assert!(project.prj_task.sub_block.lines.is_empty());
}

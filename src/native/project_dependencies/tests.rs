//! Project blocking vectors copied from `docs/task-dependencies.md` §13.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use chrono::NaiveDate;

use super::super::task_status_hooks::{
    parse_tasks, task_dependency_states, FileScan, TaskStatusType,
    TasksSettings,
};
use super::*;

fn settings() -> TasksSettings {
    TasksSettings {
        global_filter: "#task".to_string(),
        done_statuses: BTreeSet::from(['x', 'X']),
        status_types: BTreeMap::from([
            (' ', TaskStatusType::Todo),
            ('x', TaskStatusType::Done),
            ('X', TaskStatusType::Done),
            ('/', TaskStatusType::InProgress),
            ('*', TaskStatusType::OnHold),
            ('?', TaskStatusType::OnHold),
            ('-', TaskStatusType::Cancelled),
        ]),
        status_definitions: Vec::new(),
        status_settings_error: None,
    }
}

fn scan_note(path: &str, contents: &str) -> FileScan {
    FileScan {
        path: PathBuf::from("/vault").join(path),
        relative_path: PathBuf::from(path),
        contents: contents.to_string(),
        tasks: parse_tasks(contents, &settings()),
    }
}

fn project_note(body: &str) -> String {
    format!("---\ntype: [[project]]\nstatus: wip\n---\n{body}")
}

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).expect("test date")
}

fn reasons_for_project(
    files: &[FileScan],
    project_file_index: usize,
    context: &ProjectContext,
    today: NaiveDate,
) -> ProjectBlockReasons {
    let states = task_dependency_states(files, &BTreeSet::new());
    let task_index = context.project_task_index().expect("recognised project");
    project_block_reasons(
        context,
        &states[&(project_file_index, task_index)].open_dependency_ids,
        today,
    )
}

#[test]
fn recognises_only_one_open_prj_in_a_live_project_note() {
    let file = scan_note(
        "project.md",
        &project_note("- [ ] #task Ship #hide ^prj\n- [ ] #task Work ^work\n"),
    );
    let context = ProjectContext::for_note(&file);
    assert!(context.recognised);
    assert_eq!(context.project_task_index(), Some(0));

    let mixed_case = scan_note(
        "project.md",
        &project_note("- [ ] #task Ship #hide ^PRJ\n- [ ] #task Work ^work\n"),
    );
    let mixed_context = ProjectContext::for_note(&mixed_case);
    assert!(mixed_context.recognised);
    assert_eq!(mixed_context.project_task_index(), Some(0));

    for (path, contents) in [
        (
            "done/project.md",
            "---\ntype: [[project]]\nstatus: done\n---\n- [ ] #task Ship ^prj\n",
        ),
        (
            "two.md",
            &project_note("- [ ] #task One ^prj\n- [ ] #task Two ^prj\n"),
        ),
        (
            "2026/20261012.md",
            &project_note("- [ ] #task Ship ^prj\n"),
        ),
    ] {
        assert!(
            !ProjectContext::for_note(&scan_note(path, contents)).recognised,
            "{path} must not be recognised"
        );
    }
}

#[test]
fn project_reasons_come_from_the_prj_depends_on_line_and_schedule() {
    // PDB1/PDB7 (§13.14): open prerequisites and a future inline date
    // on ^prj make project reasons for its residents.
    let project = scan_note(
        "project.md",
        &project_note(
            "- [?] #task Ship #hide [dependsOn:: prerequisite] [scheduled:: 2999-01-01] ^prj\n- [ ] #task Work ^work\n",
        ),
    );
    let prerequisite = scan_note(
        "source.md",
        "---\n---\n- [ ] #task Prepare [id:: prerequisite] ^prepare\n",
    );
    let files = vec![project, prerequisite];
    let context = ProjectContext::for_note(&files[0]);
    let today = date(2026, 10, 12);
    let reasons = reasons_for_project(&files, 0, &context, today);
    assert_eq!(reasons.open_dependency_ids, ["prerequisite"]);
    assert_eq!(reasons.future_scheduled, Some(date(2999, 1, 1)));

    let resident = &files[0].tasks[1];
    let kinds =
        effective_block_reasons(resident, &[], &context, &reasons, today)
            .iter()
            .map(BlockReason::kind)
            .collect::<Vec<_>>();
    assert_eq!(kinds, ["project_dependency", "project_schedule"]);

    // §13.3: ^prj reports its own reasons, never project_* reasons.
    let prj = &files[0].tasks[0];
    let kinds = effective_block_reasons(
        prj,
        &reasons.open_dependency_ids,
        &context,
        &reasons,
        today,
    )
    .iter()
    .map(BlockReason::kind)
    .collect::<Vec<_>>();
    assert_eq!(kinds, ["local_dependency", "local_schedule"]);
}

#[test]
fn project_frontmatter_schedule_is_not_a_blocking_input() {
    let file = scan_note(
        "project.md",
        "---\ntype: [[project]]\nstatus: wip\nscheduled: 2999-01-01\n---\n- [ ] #task Ship #hide ^prj\n",
    );
    let context = ProjectContext::for_note(&file);
    assert!(context.recognised);
    assert_eq!(context.scheduled, None);
    assert!(project_block_reasons(&context, &[], date(2026, 10, 12)).is_empty());
}

#[test]
fn project_reasons_apply_to_nested_and_ref_residents_but_not_fenced_lines() {
    // PDB1 (§13.14): residence is physical, includes #ref and nested
    // tasks, and excludes fenced task-looking examples.
    let file = scan_note(
        "project.md",
        &project_note(
            "- [ ] #task Ship #hide ^prj\n- [ ] #task Ref work #ref ^ref-work\n\t- [ ] #task Nested ^nested\n```\n- [ ] #task Example ^fenced\n```\n",
        ),
    );
    let context = ProjectContext::for_note(&file);
    assert!(context.is_resident(&file.tasks[1]));
    assert!(context.is_resident(&file.tasks[2]));
    assert!(!context.is_resident(&file.tasks[0]));
    assert!(!context.is_resident(&file.tasks[3]));
}

#[test]
fn project_reasons_do_not_clear_a_residents_own_reason() {
    // PDB3 (§13.14): removing the project reason leaves the resident's
    // own future schedule in place.
    let file = scan_note(
        "project.md",
        &project_note(
            "- [ ] #task Ship #hide ^prj\n- [ ] #task Slow [scheduled:: 2999-01-01] ^slow\n",
        ),
    );
    let context = ProjectContext::for_note(&file);
    let today = date(2026, 10, 12);
    let blocked =
        project_block_reasons(&context, &["prerequisite".into()], today);
    let resident = &file.tasks[1];
    let kinds =
        effective_block_reasons(resident, &[], &context, &blocked, today)
            .iter()
            .map(BlockReason::kind)
            .collect::<Vec<_>>();
    assert_eq!(kinds, ["local_schedule", "project_dependency"]);

    let cleared = ProjectBlockReasons::default();
    let kinds =
        effective_block_reasons(resident, &[], &context, &cleared, today)
            .iter()
            .map(BlockReason::kind)
            .collect::<Vec<_>>();
    assert_eq!(kinds, ["local_schedule"]);
}

#[test]
fn project_reasons_ignore_unresolved_or_closed_dependencies() {
    let project = scan_note(
        "project.md",
        &project_note(
            "- [ ] #task Ship #hide [dependsOn:: missing, finished] ^prj\n",
        ),
    );
    let finished = scan_note(
        "source.md",
        "---\n---\n- [x] #task Finished [id:: finished] ^finished\n",
    );
    let files = vec![project, finished];
    let context = ProjectContext::for_note(&files[0]);
    assert!(
        reasons_for_project(&files, 0, &context, date(2026, 10, 12)).is_empty()
    );
}

#[test]
fn project_reason_path_is_the_project_note_path() {
    let file = scan_note(
        "areas/project.md",
        &project_note("- [ ] #task Ship #hide ^prj\n- [ ] #task Work ^work\n"),
    );
    let context = ProjectContext::for_note(&file);
    let reasons = project_block_reasons(
        &context,
        &["required".into()],
        date(2026, 10, 12),
    );
    let inherited = effective_block_reasons(
        &file.tasks[1],
        &[],
        &context,
        &reasons,
        date(2026, 10, 12),
    );
    assert_eq!(
        inherited,
        vec![BlockReason::ProjectDependency {
            project: PathBuf::from("areas/project.md"),
            open_ids: vec!["required".into()],
        }]
    );
}

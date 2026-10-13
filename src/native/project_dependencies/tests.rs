//! Project blocking vectors copied from `docs/task-dependencies.md` §13.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use super::super::task_status_hooks::{
    parse_tasks, task_dependency_states, FileScan, TaskStatusType,
    TasksSettings,
};
use super::tree::{
    ancestor_chain, project_ancestors, project_parent_target,
    resolve_project_parent,
};
use super::*;
use crate::native::vault_links::NoteIndex;

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
    assert!(!mixed_context.recognised);
    assert_eq!(mixed_context.project_task_index(), None);

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
        effective_block_reasons(resident, &[], &context, &reasons, &[], today)
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
        &[],
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
        effective_block_reasons(resident, &[], &context, &blocked, &[], today)
            .iter()
            .map(BlockReason::kind)
            .collect::<Vec<_>>();
    assert_eq!(kinds, ["local_schedule", "project_dependency"]);

    let cleared = ProjectBlockReasons::default();
    let kinds =
        effective_block_reasons(resident, &[], &context, &cleared, &[], today)
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
        &[],
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

#[test]
fn project_parent_targets_resolve_scalar_alias_path_and_heading_forms() {
    // PDS15: aliases and headings are removed while the target path remains.
    let note = |parent: &str| {
        format!(
            "---\ntype: [[project]]\nstatus: wip\nparent: {parent}\n---\n- [ ] #task Work ^work\n"
        )
    };
    let children = [
        ("alias.md", note("\"[[N|Alias]]\"")),
        ("path.md", note("\"[[dir/N]]\"")),
        ("heading.md", note("\"[[N#Heading]]\"")),
    ];
    let index = NoteIndex::from_paths([
        PathBuf::from("N.md"),
        PathBuf::from("alias.md"),
        PathBuf::from("heading.md"),
    ]);
    assert_eq!(
        resolve_project_parent(Path::new("alias.md"), &children[0].1, &index,),
        Some(PathBuf::from("N.md"))
    );
    assert_eq!(
        resolve_project_parent(
            Path::new("path.md"),
            &children[1].1,
            &NoteIndex::from_paths([
                PathBuf::from("dir/N.md"),
                PathBuf::from("path.md"),
            ]),
        ),
        Some(PathBuf::from("dir/N.md"))
    );
    assert_eq!(
        resolve_project_parent(Path::new("heading.md"), &children[2].1, &index,),
        Some(PathBuf::from("N.md"))
    );
    assert_eq!(
        project_parent_target(&note("[[N]], [[Other]]")),
        None,
        "list-form parent values are not scalar wikilinks"
    );
    assert_eq!(
        resolve_project_parent(Path::new("N.md"), &note("[[N]]"), &index,),
        None,
        "a self-parent is ignored"
    );
    let ambiguous = NoteIndex::from_paths([
        PathBuf::from("one/N.md"),
        PathBuf::from("two/N.md"),
        PathBuf::from("child.md"),
    ]);
    assert_eq!(
        resolve_project_parent(
            Path::new("child.md"),
            &note("[[N]]"),
            &ambiguous,
        ),
        None,
        "ambiguous basename parents are ignored"
    );
}

#[test]
fn project_tree_resolves_nearest_ancestors_and_stops_at_nonprojects() {
    let files = vec![
        scan_note(
            "parent.md",
            &project_note("- [ ] #task Parent ^prj\n"),
        ),
        scan_note(
            "child.md",
            "---\ntype: [[project]]\nstatus: wip\nparent: [[parent]]\n---\n- [ ] #task Child ^prj\n",
        ),
        scan_note(
            "grandchild.md",
            "---\ntype: [[project]]\nstatus: wip\nparent: [[child]]\n---\n- [ ] #task Grandchild ^prj\n",
        ),
        scan_note(
            "area.md",
            "---\ntype: [[area]]\n---\n- [ ] #task Area ^area\n",
        ),
        scan_note(
            "area-child.md",
            "---\ntype: [[project]]\nstatus: wip\nparent: [[area]]\n---\n- [ ] #task Area child ^prj\n",
        ),
        scan_note(
            "terminal.md",
            "---\ntype: [[project]]\nstatus: done\nparent: [[parent]]\n---\n- [ ] #task Terminal ^prj\n",
        ),
        scan_note(
            "terminal-child.md",
            "---\ntype: [[project]]\nstatus: wip\nparent: [[terminal]]\n---\n- [ ] #task Child of terminal ^prj\n",
        ),
        scan_note(
            "two-prj.md",
            "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task First ^prj\n- [ ] #task Second ^prj\n",
        ),
        scan_note(
            "two-prj-child.md",
            "---\ntype: [[project]]\nstatus: wip\nparent: [[two-prj]]\n---\n- [ ] #task Child ^prj\n",
        ),
        scan_note(
            "2026/20260716.md",
            "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Daily ^prj\n",
        ),
        scan_note(
            "daily-child.md",
            "---\ntype: [[project]]\nstatus: wip\nparent: [[2026/20260716]]\n---\n- [ ] #task Child ^prj\n",
        ),
    ];
    let contexts = files
        .iter()
        .map(ProjectContext::for_note)
        .collect::<Vec<_>>();
    let reasons = vec![ProjectBlockReasons::default(); files.len()];
    let index = NoteIndex::from_paths(
        files.iter().map(|file| file.relative_path.clone()),
    );
    let mut tree = ProjectTree::build(&files, &contexts, &reasons, &index);

    assert_eq!(tree.parent_index(1), Some(0));
    assert_eq!(tree.children(0), &[1]);
    assert_eq!(tree.ancestors(2), &[1, 0]);
    assert_eq!(tree.descendants(0), &[1, 2]);
    assert!(
        tree.ancestors(4).is_empty(),
        "PDS6: area parent stops the chain"
    );
    assert!(
        tree.ancestors(6).is_empty(),
        "PDS7: terminal project stops the chain"
    );
    assert!(
        tree.ancestors(8).is_empty(),
        "PDS6: two-^prj parent stops the chain"
    );
    assert!(
        tree.ancestors(10).is_empty(),
        "PDS6: canonical daily parent stops the chain"
    );

    let mut own_reasons = reasons;
    own_reasons[0] = ProjectBlockReasons {
        open_dependency_ids: vec!["outside".into()],
        future_scheduled: None,
    };
    tree.set_project_reasons(&files, own_reasons);
    assert_eq!(
        tree.inherited(2)[0],
        (
            PathBuf::from("parent.md"),
            ProjectBlockReasons {
                open_dependency_ids: vec!["outside".into()],
                future_scheduled: None,
            }
        )
    );
    assert_eq!(tree.inherited(4).len(), 0);
}

#[test]
fn project_tree_cycle_excludes_self_and_reports_the_cycle_once() {
    let files = vec![
        scan_note(
            "A.md",
            "---\ntype: [[project]]\nstatus: wip\nparent: [[B]]\n---\n- [ ] #task A ^prj\n",
        ),
        scan_note(
            "B.md",
            "---\ntype: [[project]]\nstatus: wip\nparent: [[A]]\n---\n- [ ] #task B ^prj\n",
        ),
    ];
    let contexts = files
        .iter()
        .map(ProjectContext::for_note)
        .collect::<Vec<_>>();
    let reasons = vec![ProjectBlockReasons::default(); files.len()];
    let index = NoteIndex::from_paths(
        files.iter().map(|file| file.relative_path.clone()),
    );
    let mut tree = ProjectTree::build(&files, &contexts, &reasons, &index);
    assert_eq!(tree.cycles(), &[vec![0, 1]]);
    assert_eq!(tree.ancestors(0), &[1]);
    assert_eq!(tree.ancestors(1), &[0]);

    let mut own_reasons = reasons;
    own_reasons[0].open_dependency_ids = vec!["outside".into()];
    tree.set_project_reasons(&files, own_reasons);
    assert_eq!(tree.inherited(0).len(), 0);
    assert_eq!(tree.inherited(1)[0].0, PathBuf::from("A.md"));
}

#[test]
fn ancestor_chain_is_read_on_demand_cycle_safe_and_depth_limited() {
    let index = NoteIndex::from_paths([
        PathBuf::from("child.md"),
        PathBuf::from("parent.md"),
        PathBuf::from("grandparent.md"),
    ]);
    let parent = project_note("- [ ] #task Parent ^prj\n");
    let grandparent = "---\ntype: [[project]]\nstatus: wip\nparent: [[child]]\n---\n- [ ] #task Grandparent ^prj\n".to_string();
    let contents = "---\ntype: [[project]]\nstatus: wip\nparent: [[parent]]\n---\n- [ ] #task Child ^prj\n";
    let chain = ancestor_chain(
        Path::new("child.md"),
        contents,
        &index,
        |path| match path.to_str() {
            Some("parent.md") => Some(parent.clone()),
            Some("grandparent.md") => Some(grandparent.clone()),
            _ => None,
        },
        16,
    );
    assert_eq!(chain, [PathBuf::from("parent.md")]);

    let linear_index = NoteIndex::from_paths([
        PathBuf::from("child.md"),
        PathBuf::from("parent.md"),
        PathBuf::from("grandparent.md"),
    ]);
    let parent = "---\ntype: [[project]]\nstatus: wip\nparent: [[grandparent]]\n---\n- [ ] #task Parent ^prj\n".to_string();
    let chain = ancestor_chain(
        Path::new("child.md"),
        contents,
        &linear_index,
        |path| match path.to_str() {
            Some("parent.md") => Some(parent.clone()),
            Some("grandparent.md") => Some(grandparent.clone()),
            _ => None,
        },
        1,
    );
    assert_eq!(chain, [PathBuf::from("parent.md")]);
}

#[test]
fn project_ancestors_stop_at_the_first_non_project() {
    // PDS7: a terminal or area parent breaks the chain.
    let index = NoteIndex::from_paths([
        PathBuf::from("child.md"),
        PathBuf::from("area.md"),
        PathBuf::from("root.md"),
    ]);
    let child = "---\ntype: [[project]]\nstatus: wip\nparent: [[area]]\n---\n- [ ] #task Child ^prj\n";
    let area = "---\ntype: [[area]]\n---\n".to_string();
    let root = project_note("- [ ] #task Root ^prj\n");
    let chain = project_ancestors(
        Path::new("child.md"),
        child,
        &index,
        |path| match path.to_str() {
            Some("area.md") => Some(area.clone()),
            Some("root.md") => Some(root.clone()),
            _ => None,
        },
        |path, contents| {
            let file = scan_note(path.to_str().unwrap_or_default(), contents);
            ProjectContext::for_note(&file).recognised
        },
    );
    assert!(chain.is_empty(), "{chain:?}");
}

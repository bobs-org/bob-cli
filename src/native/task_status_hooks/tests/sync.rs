//! Daily, reference, parse, and status-transition tests.
use super::*;
use crate::native::project_dependencies::{
    ProjectBlockReasons, ProjectContext, ProjectTree,
};

#[test]
fn previous_daily_selection_uses_latest_canonical_earlier_date() {
    let vault = Path::new("/vault");
    let paths = [
        "/vault/2025/20251231.md",
        "/vault/2026/20260101.md",
        "/vault/2026/20260102.md",
        "/vault/2026/20261301.md",
        "/vault/2026/20260101_day.md",
        "/vault/Other/20260101.md",
        "/vault/2027/20270101.md",
    ]
    .map(PathBuf::from);

    assert_eq!(
        previous_daily_path(vault, &paths, date(2026, 1, 2)),
        Some(PathBuf::from("/vault/2026/20260101.md"))
    );
    assert_eq!(
        previous_daily_path(vault, &paths, date(2026, 1, 1),),
        Some(PathBuf::from("/vault/2025/20251231.md"))
    );
    assert_eq!(
        previous_daily_path(
            vault,
            &[PathBuf::from("/vault/2026/20260701.md")],
            date(2026, 7, 21),
        ),
        Some(PathBuf::from("/vault/2026/20260701.md"))
    );
    assert_eq!(
        previous_daily_path(
            vault,
            &[PathBuf::from("/vault/2026/20260721.md")],
            date(2026, 7, 21),
        ),
        None
    );
}

#[test]
fn dated_day_file_overrides_effective_anchor_and_malformed_name_falls_back() {
    assert_eq!(
        daily_anchor_date(
            Path::new("/fixtures/20251231.md"),
            date(2026, 7, 21),
        ),
        date(2025, 12, 31)
    );
    assert_eq!(
        daily_anchor_date(
            Path::new("/fixtures/20251230_day.md"),
            date(2026, 7, 21),
        ),
        date(2025, 12, 30)
    );
    assert_eq!(
        daily_anchor_date(
            Path::new("/fixtures/not-a-day.md"),
            date(2026, 7, 21),
        ),
        date(2026, 7, 21)
    );
}

#[test]
fn recent_links_include_completed_live_links_but_exclude_retired_links() {
    let lines = [
        "- [ ] Open",
        "  - [[tasks#^open]]",
        "  - ~~[[tasks#^retired-open]]~~",
        "- [x] Completed",
        "  - 🍅 [[tasks#^completed-live|alias]]",
        "  - ~~![[tasks#^retired-completed]]~~",
        "  ```md",
        "  - [[tasks#^fenced]]",
        "  ```",
    ];
    let model = scan_pomodoros(&lines, 0..lines.len());

    assert_eq!(
        model.recent_references,
        BTreeSet::from([
            reference("tasks", "completed-live"),
            reference("tasks", "open"),
        ])
    );
}

#[test]
fn same_note_recent_links_resolve_in_each_daily_context() {
    let references = BTreeSet::from([reference("", "local")]);
    let index = NoteIndex::from_paths([
        PathBuf::from("2026/20260710.md"),
        PathBuf::from("2026/20260721.md"),
    ]);
    let blocks = BTreeMap::from([
        (
            (PathBuf::from("2026/20260710.md"), "local".to_string()),
            vec!['/'],
        ),
        (
            (PathBuf::from("2026/20260721.md"), "local".to_string()),
            vec!['/'],
        ),
    ]);
    let archive_catalog = ArchiveReferenceCatalog::default();
    let resolver = TaskReferenceResolver {
        note_index: &index,
        task_blocks: &blocks,
        archive_catalog: &archive_catalog,
    };
    let settings = test_settings();

    let previous = resolve_recent_references(
        &references,
        Some(Path::new("2026/20260710.md")),
        &resolver,
        &settings,
        &mut Vec::new(),
    );
    let current = resolve_recent_references(
        &references,
        Some(Path::new("2026/20260721.md")),
        &resolver,
        &settings,
        &mut Vec::new(),
    );

    assert_eq!(
        previous,
        BTreeSet::from([(
            PathBuf::from("2026/20260710.md"),
            "local".to_string(),
        )])
    );
    assert_eq!(
        current,
        BTreeSet::from([(
            PathBuf::from("2026/20260721.md"),
            "local".to_string(),
        )])
    );
}

#[test]
fn note_kind_uses_shared_area_and_project_frontmatter_predicates() {
    assert_eq!(note_kind("---\ntype: [[area]]\n---\n"), NoteKind::Area);
    assert_eq!(
        note_kind("---\ntype: \"[[project]]\"\n---\n"),
        NoteKind::Project
    );
    assert_eq!(note_kind("---\ntype: '[[area]]'\n---\n"), NoteKind::Area);
    assert_eq!(note_kind("---\ntype: [[Area]]\n---\n"), NoteKind::Other);
}

#[test]
fn resolves_exact_and_unique_case_insensitive_basenames() {
    let index = NoteIndex::from_paths([
        PathBuf::from("Areas/Home.md"),
        PathBuf::from("Projects/Alpha.md"),
    ]);
    assert_eq!(
        index.resolve(None, "Projects/Alpha"),
        Some(PathBuf::from("Projects/Alpha.md"))
    );
    assert_eq!(
        index.resolve(None, "alpha.md"),
        Some(PathBuf::from("Projects/Alpha.md"))
    );
    assert_eq!(
        index.resolve(None, "HOME"),
        Some(PathBuf::from("Areas/Home.md"))
    );
}

#[test]
fn ambiguous_basename_does_not_resolve() {
    let index = NoteIndex::from_paths([
        PathBuf::from("Areas/Home.md"),
        PathBuf::from("Projects/home.md"),
    ]);
    assert_eq!(index.resolve(None, "home"), None);
}

#[test]
fn dotted_note_names_keep_the_full_basename() {
    let index = NoteIndex::from_paths([
        PathBuf::from("Notes.md"),
        PathBuf::from("Notes.v2.md"),
    ]);
    assert_eq!(
        index.resolve(None, "Notes.v2"),
        Some(PathBuf::from("Notes.v2.md"))
    );
    assert_eq!(
        target_to_markdown_path("Notes.v2"),
        Some(PathBuf::from("Notes.v2.md"))
    );
}

#[test]
fn fenced_column_zero_content_does_not_end_dependency_scan() {
    let settings = test_settings();
    let a_contents = "- [ ] #task A [dependsOn:: B__b] ^a\n  ```\nnot a list item\n  ```\n  - ⛓️ **DEPENDS ON:** [[B#^b]]\n";
    let b_contents = "- [ ] #task B [id:: B__b] ^b\n";
    let files = vec![
        FileScan {
            path: PathBuf::from("A.md"),
            relative_path: PathBuf::from("A.md"),
            contents: a_contents.to_string(),
            tasks: parse_tasks(a_contents, &settings),
        },
        FileScan {
            path: PathBuf::from("B.md"),
            relative_path: PathBuf::from("B.md"),
            contents: b_contents.to_string(),
            tasks: parse_tasks(b_contents, &settings),
        },
    ];
    let index = NoteIndex::from_paths(
        files.iter().map(|file| file.relative_path.clone()),
    );
    let blocks = task_blocks(&files);
    let tree = project_tree_for(&files, &index);
    let edges = dependency_edges(&files, &index, &blocks, &tree);
    assert!(edges[&(PathBuf::from("A.md"), "a".to_string())]
        .contains(&(PathBuf::from("B.md"), "b".to_string())));
}

#[test]
fn parses_task_markers_and_preserves_status_offsets() {
    let contents = concat!(
        "  1. [ ] #task Todo ^todo\r\n",
        "* [*] #task Next ^next\r\n",
        "+ [/] #task Working ^work\r\n",
        "- [*] not a task ^ignored\r\n",
    );
    let tasks = parse_tasks(contents, &test_settings());
    assert_eq!(tasks.len(), 3);
    assert_eq!(tasks[0].status, ' ');
    assert_eq!(tasks[0].block_id.as_deref(), Some("todo"));
    assert_eq!(tasks[0].description, "Todo");
    assert_eq!(&contents[tasks[1].status_byte_offset..][..1], "*");
    assert_eq!(tasks[2].status, '/');
}

#[test]
fn parses_bracket_and_parenthesized_task_dependency_metadata() {
    let contents = concat!(
        "- [ ] #task Bracket [id:: alpha] [dependsOn:: root, other] ^a\n",
        "- [?] #task Parenthesized (id:: beta) (dependsOn:: root) #tag ^b\n",
        "- [ ] #task Invalid [id:: bad value] [dependsOn:: root, bad value] ^c\n",
    );
    let tasks = parse_tasks(contents, &test_settings());
    assert_eq!(tasks[0].task_id.as_deref(), Some("alpha"));
    assert_eq!(tasks[0].depends_on, ["root", "other"]);
    assert_eq!(tasks[1].task_id.as_deref(), Some("beta"));
    assert_eq!(tasks[1].depends_on, ["root"]);
    assert_eq!(tasks[1].status_type, TaskStatusType::OnHold);
    assert!(tasks[1].status_recognized);
    assert!(tasks[2].task_id.is_none());
    assert!(tasks[2].depends_on.is_empty());
}

#[test]
fn parses_only_calendar_valid_scheduled_metadata_in_supported_forms() {
    let contents = concat!(
        "- [ ] #task Bracket [scheduled:: 2026-07-17] [id:: alpha] ^a\n",
        "- [ ] #task Parenthesized (dependsOn:: root) (scheduled::   2026-07-18  ) #tag ^b\n",
        "- [ ] #task Flexible order [created:: 2026-07-01] [scheduled:: 2026-07-19] [due:: 2026-07-20] ^c\n",
        "- [ ] #task Impossible [scheduled:: 2026-02-30] ^d\n",
        "- [ ] #task Bad shape [scheduled:: 2026-7-20] ^e\n",
        "- [ ] #task Bad delimiter [scheduled :: 2026-07-20] ^f\n",
    );
    let tasks = parse_tasks(contents, &test_settings());
    assert_eq!(tasks[0].scheduled, NaiveDate::from_ymd_opt(2026, 7, 17));
    assert_eq!(tasks[1].scheduled, NaiveDate::from_ymd_opt(2026, 7, 18));
    assert_eq!(tasks[2].scheduled, NaiveDate::from_ymd_opt(2026, 7, 19));
    assert!(tasks[3..].iter().all(|task| task.scheduled.is_none()));
}

#[test]
fn future_schedule_uses_the_calendar_day_after_the_anchor() {
    let anchor = NaiveDate::from_ymd_opt(2026, 7, 16).unwrap();
    for (value, future) in [
        ("2026-07-15", false),
        ("2026-07-16", false),
        ("2026-07-17", true),
        ("2027-01-01", true),
    ] {
        let task = parse_tasks(
            &format!("- [ ] #task Boundary [scheduled:: {value}]\n"),
            &test_settings(),
        )
        .remove(0);
        assert_eq!(
            task.scheduled.is_some_and(|scheduled| scheduled > anchor),
            future,
            "{value}"
        );
    }
}

#[test]
fn task_dependency_index_matches_tasks_duplicate_and_missing_id_semantics() {
    let mut settings = test_settings();
    settings.status_types.insert('~', TaskStatusType::NonTask);
    let contents = concat!(
        "- [x] #task Duplicate done [id:: duplicate] ^done\n",
        "- [ ] #task Duplicate open [id:: duplicate] ^open\n",
        "- [x] #task Closed [id:: closed] ^closed\n",
        "- [~] #task Non-task [id:: non-task] ^non-task\n",
        "- [!] #task Unknown [id:: unknown] ^unknown\n",
        "- [ ] #task Self [id:: self] [dependsOn:: self] ^self\n",
        "- [ ] #task Parent [dependsOn:: duplicate, closed, non-task, unknown, missing] ^parent\n",
    );
    let files = vec![FileScan {
        path: PathBuf::from("tasks.md"),
        relative_path: PathBuf::from("tasks.md"),
        contents: contents.to_string(),
        tasks: parse_tasks(contents, &settings),
    }];
    let states = task_dependency_states(&files, &BTreeSet::new());
    assert_eq!(states[&(0, 5)].open_dependency_ids, ["self"]);
    assert_eq!(states[&(0, 6)].open_dependency_ids, ["duplicate"]);
    assert_eq!(states[&(0, 6)].unresolved_dependency_ids, ["missing"]);
}

#[test]
fn replacement_changes_only_status_and_preserves_crlf() {
    let contents = "  - [ ] #task Keep everything ^id\r\n";
    let task = parse_tasks(contents, &test_settings()).remove(0);
    let mut changed = contents.to_string();
    changed.replace_range(
        task.status_byte_offset..task.status_byte_offset + 1,
        "*",
    );
    assert_eq!(changed, "  - [*] #task Keep everything ^id\r\n");
}

#[test]
fn transition_matrix_promotes_monotonically_and_clears_only_unreferenced_next()
{
    assert_eq!(
        transition(' ', Some(RankedStatus::Next)),
        Transition::MarkNext
    );
    assert_eq!(
        transition(' ', Some(RankedStatus::InProgress)),
        Transition::MarkInProgress
    );
    assert_eq!(
        transition('*', Some(RankedStatus::InProgress)),
        Transition::MarkInProgress
    );
    assert_eq!(transition(' ', None), Transition::Unchanged);
    assert_eq!(transition('*', None), Transition::Clear);
    assert_eq!(
        transition('*', Some(RankedStatus::Next)),
        Transition::KeptNext
    );
    assert_eq!(
        transition('/', Some(RankedStatus::Next)),
        Transition::KeptInProgress
    );
    assert_eq!(transition('/', None), Transition::Unchanged);
    for status in ['x', 'X', '-', '!'] {
        assert_eq!(
            transition(status, Some(RankedStatus::InProgress)),
            Transition::Unchanged
        );
        assert_eq!(transition(status, None), Transition::Unchanged);
    }
}

#[test]
fn blocked_transition_precedence_and_recovery_are_explicit() {
    let settings = test_settings();
    let ready = parse_tasks("- [ ] #task Ready\n", &settings).remove(0);
    let next = parse_tasks("- [*] #task Next\n", &settings).remove(0);
    let working = parse_tasks("- [/] #task Working\n", &settings).remove(0);
    let blocked = parse_tasks("- [?] #task Blocked\n", &settings).remove(0);
    let done = parse_tasks("- [x] #task Done\n", &settings).remove(0);
    assert_eq!(
        task_transition(
            &ready,
            Some(RankedStatus::InProgress),
            None,
            false,
            true,
            false,
        ),
        Transition::MarkBlocked
    );
    assert_eq!(
        task_transition(&next, None, None, false, true, false),
        Transition::MarkBlocked
    );
    assert_eq!(
        task_transition(
            &blocked,
            Some(RankedStatus::InProgress),
            Some(RankedStatus::InProgress),
            true,
            true,
            false,
        ),
        Transition::Unchanged
    );
    assert_eq!(
        task_transition(
            &blocked,
            None,
            Some(RankedStatus::InProgress),
            true,
            false,
            false,
        ),
        Transition::Unblock(RankedStatus::InProgress)
    );
    assert_eq!(
        task_transition(&blocked, None, None, false, false, false),
        Transition::Unblock(RankedStatus::Ready)
    );
    assert_eq!(
        task_transition(
            &done,
            Some(RankedStatus::InProgress),
            Some(RankedStatus::InProgress),
            true,
            true,
            false,
        ),
        Transition::Unchanged
    );
    assert_eq!(
        task_transition(&working, None, None, false, false, true),
        Transition::Unchanged
    );
    assert_eq!(
        task_transition(&working, None, None, false, false, false),
        Transition::Unchanged
    );
    assert_eq!(
        task_transition(&working, None, None, false, true, true),
        Transition::MarkBlocked
    );
    assert_eq!(
        task_transition(
            &next,
            None,
            Some(RankedStatus::Next),
            true,
            false,
            true,
        ),
        Transition::KeptNext
    );
    assert_eq!(
        task_transition(
            &ready,
            None,
            Some(RankedStatus::InProgress),
            true,
            false,
            false,
        ),
        Transition::Unchanged
    );
}

#[test]
fn sticky_lanes_keep_next_and_in_progress_outside_daily_notes() {
    let settings = test_settings();
    let ready = parse_tasks("- [ ] #task Ready\n", &settings).remove(0);
    let next = parse_tasks("- [*] #task Next\n", &settings).remove(0);
    let working = parse_tasks("- [/] #task Working\n", &settings).remove(0);
    let blocked = parse_tasks("- [?] #task Blocked\n", &settings).remove(0);
    // 1. Unlinked [*] in an ordinary note stays [*] without counting as kept.
    assert_eq!(
        task_transition(&next, None, None, false, false, false),
        Transition::Unchanged
    );
    // ... even when directly recent: still Unchanged, never KeptNext.
    assert_eq!(
        task_transition(
            &next,
            None,
            Some(RankedStatus::Next),
            true,
            false,
            false
        ),
        Transition::Unchanged
    );
    // 2. Unlinked [*] in an area/project note also stays [*]; the lane rule
    // does not consult note kind, so daily=false covers both.
    assert_eq!(
        task_transition(&next, None, None, true, false, false),
        Transition::Unchanged
    );
    // 3. Area/project [/] with no recent activity stays [/].
    assert_eq!(
        task_transition(&working, None, None, false, false, false),
        Transition::Unchanged
    );
    assert_eq!(
        task_transition(&working, None, None, true, false, false),
        Transition::Unchanged
    );
    // 4. A ^gtd-style [*] in the daily note still clears without grace, and
    // is kept only while directly recent.
    assert_eq!(
        task_transition(&next, None, None, false, false, true),
        Transition::Clear
    );
    assert_eq!(
        task_transition(
            &next,
            None,
            Some(RankedStatus::Next),
            true,
            false,
            true
        ),
        Transition::KeptNext
    );
    // 5. Blocked still overrides [*] and [/], and recovers to Ready when the
    // task was never linked.
    assert_eq!(
        task_transition(&next, None, None, false, true, false),
        Transition::MarkBlocked
    );
    assert_eq!(
        task_transition(&working, None, None, false, true, false),
        Transition::MarkBlocked
    );
    assert_eq!(
        task_transition(&blocked, None, None, false, false, false),
        Transition::Unblock(RankedStatus::Ready)
    );
    // 6. Ready -> Next on link is unchanged.
    assert_eq!(
        task_transition(
            &ready,
            Some(RankedStatus::Next),
            None,
            false,
            false,
            false
        ),
        Transition::MarkNext
    );
}

#[test]
fn desired_statuses_merge_parents_and_propagate_stronger_intermediates_through_cycles(
) {
    let ready_root = identity("ready-root");
    let working_root = identity("working-root");
    let shared = identity("shared");
    let stronger_mid = identity("stronger-mid");
    let leaf = identity("leaf");
    let direct = BTreeSet::from([ready_root.clone(), working_root.clone()]);
    let edges = BTreeMap::from([
        (ready_root.clone(), BTreeSet::from([shared.clone()])),
        (working_root.clone(), BTreeSet::from([shared.clone()])),
        (shared.clone(), BTreeSet::from([stronger_mid.clone()])),
        (stronger_mid.clone(), BTreeSet::from([leaf.clone()])),
        (leaf.clone(), BTreeSet::from([shared.clone()])),
    ]);
    let task_blocks = BTreeMap::from([
        (ready_root.clone(), vec![' ']),
        (working_root.clone(), vec!['*']),
        (shared.clone(), vec![' ']),
        (stronger_mid.clone(), vec!['/']),
        (leaf.clone(), vec!['*']),
    ]);

    let desired =
        desired_statuses(&direct, &edges, &task_blocks, &BTreeSet::new());

    assert_eq!(desired[&ready_root], RankedStatus::Next);
    assert_eq!(desired[&working_root], RankedStatus::Next);
    for identity in [&shared, &stronger_mid, &leaf] {
        assert_eq!(desired[identity], RankedStatus::InProgress);
    }
}

#[test]
fn recovery_rank_defaults_blocked_roots_to_next_and_propagates_in_progress() {
    let blocked_root = identity("blocked-root");
    let working_child = identity("working-child");
    let blocked_leaf = identity("blocked-leaf");
    let edges = BTreeMap::from([
        (
            blocked_root.clone(),
            BTreeSet::from([working_child.clone()]),
        ),
        (
            working_child.clone(),
            BTreeSet::from([blocked_leaf.clone()]),
        ),
    ]);
    let task_blocks = BTreeMap::from([
        (blocked_root.clone(), vec!['?']),
        (working_child.clone(), vec!['/']),
        (blocked_leaf.clone(), vec!['?']),
    ]);

    let recovery = desired_statuses(
        &BTreeSet::from([blocked_root.clone()]),
        &edges,
        &task_blocks,
        &BTreeSet::new(),
    );

    assert_eq!(recovery[&blocked_root], RankedStatus::Next);
    assert_eq!(recovery[&working_child], RankedStatus::InProgress);
    assert_eq!(recovery[&blocked_leaf], RankedStatus::InProgress);
}

fn edge_scan(entries: &[(&str, &str)]) -> ScannedEdges {
    let settings = test_settings();
    let files = entries
        .iter()
        .map(|(path, contents)| FileScan {
            path: PathBuf::from(path),
            relative_path: PathBuf::from(path),
            contents: contents.to_string(),
            tasks: parse_tasks(contents, &settings),
        })
        .collect::<Vec<_>>();
    let index = NoteIndex::from_paths(
        files.iter().map(|file| file.relative_path.clone()),
    );
    let blocks = task_blocks(&files);
    let tree = project_tree_for(&files, &index);
    let edges = dependency_edges(&files, &index, &blocks, &tree);
    ScannedEdges { edges }
}

fn project_tree_for(files: &[FileScan], index: &NoteIndex) -> ProjectTree {
    let contexts = files
        .iter()
        .map(ProjectContext::for_note)
        .collect::<Vec<_>>();
    let reasons = vec![ProjectBlockReasons::default(); files.len()];
    ProjectTree::build(files, &contexts, &reasons, index)
}

struct ScannedEdges {
    edges: BTreeMap<(PathBuf, String), BTreeSet<(PathBuf, String)>>,
}

fn edge(source: &str, block: &str) -> (PathBuf, String) {
    (PathBuf::from(source), block.to_string())
}

// Promotion edges (`docs/task-dependencies.md` §5): resolved Depends-On
// line links are edges, same-note and cross-note.
#[test]
fn dependency_line_links_are_promotion_edges() {
    let scanned = edge_scan(&[
        (
            "a.md",
            "- [ ] #task Dependent [dependsOn:: a__one, b__two] ^dep\n  - ⛓️ **DEPENDS ON:** [[#^one]] • [[b#^two]]\n- [ ] #task One [id:: a__one] ^one\n",
        ),
        ("b.md", "- [ ] #task Two [id:: b__two] ^two\n"),
    ]);
    assert_eq!(
        scanned.edges.get(&edge("a.md", "dep")),
        Some(&BTreeSet::from([edge("a.md", "one"), edge("b.md", "two"),]))
    );
}

// Sole block links outside the Depends-On line are content, even when
// their targets' ids remain in the dependent's field.
#[test]
fn child_links_are_not_dependency_edges() {
    let scanned = edge_scan(&[
        (
            "a.md",
            "- [ ] #task Dependent [dependsOn:: x__e, x__p, x__s] ^dep\n  - ![[x#^e]]\n  - [[x#^p]]\n  - ~~[[x#^s]]~~\n",
        ),
        (
            "x.md",
            "- [ ] #task E [id:: x__e] ^e\n- [ ] #task P [id:: x__p] ^p\n- [ ] #task S [id:: x__s] ^s\n",
        ),
    ]);
    assert!(scanned.edges.get(&edge("a.md", "dep")).is_none());
}

// A `#^ref` reading embed is content, not an edge
// (`docs/task-dependencies.md` §5).
#[test]
fn reading_embeds_are_not_promotion_edges() {
    let scanned = edge_scan(&[(
        "a.md",
        "- [ ] #task Dependent ^dep\n  - ![[#^ref]]\n\nReference block only. ^ref\n",
    )]);
    assert!(scanned.edges.get(&edge("a.md", "dep")).is_none());
}

// Archive targets are history, never edges
// (`docs/task-dependencies.md` §4.3).
#[test]
fn archive_targets_are_not_promotion_edges() {
    let scanned = edge_scan(&[(
        "a.md",
        "- [ ] #task Dependent [dependsOn:: done__Archive__old] ^dep\n  - ⛓️ **DEPENDS ON:** [[done/Archive#^old]]\n  - ![[done/Archive#^old]]\n",
    )]);
    assert!(scanned.edges.get(&edge("a.md", "dep")).is_none());
}

// ---- cli-reconcile-block (`docs/task-dependencies.md` §13) ----

use crate::native::project_dependencies::{
    effective_block_reasons, project_block_reasons, BlockReason,
};

const PROJECT_FRONTMATTER: &str =
    "---\ntype: \"[[project]]\"\nstatus: wip\n---\n";

fn project_file(path: &str, body: &str) -> FileScan {
    let settings = test_settings();
    let contents = format!("{PROJECT_FRONTMATTER}{body}");
    FileScan {
        path: PathBuf::from(path),
        relative_path: PathBuf::from(path),
        tasks: parse_tasks(&contents, &settings),
        contents,
    }
}

fn empty_reports() -> (
    Vec<ProjectBlockedTagChange>,
    Vec<ProjectBlockedTagChange>,
    Vec<UnblockedFieldChange>,
    Vec<UnblockedFieldChange>,
) {
    (Vec::new(), Vec::new(), Vec::new(), Vec::new())
}

// ADJ-11 / PDB9: a `^prj` target is never raised into a lane, but the
// walk continues through it to that project's prerequisites.
#[test]
fn desired_statuses_walk_through_project_tasks_without_raising_them() {
    let dep = identity("dep");
    let prj = (PathBuf::from("proj.md"), "prj".to_string());
    let pre = identity("pre");
    let direct = BTreeSet::from([dep.clone()]);
    let edges = BTreeMap::from([
        (dep.clone(), BTreeSet::from([prj.clone()])),
        (prj.clone(), BTreeSet::from([pre.clone()])),
    ]);
    let task_blocks = BTreeMap::from([
        (dep.clone(), vec![' ']),
        (prj.clone(), vec!['?']),
        (pre.clone(), vec![' ']),
    ]);
    let project_tasks = BTreeSet::from([prj.clone()]);

    let desired =
        desired_statuses(&direct, &edges, &task_blocks, &project_tasks);

    assert_eq!(desired.get(&dep), Some(&RankedStatus::Next));
    assert_eq!(desired.get(&pre), Some(&RankedStatus::Next));
    assert!(!desired.contains_key(&prj));
}

// direct_prj_link = raise: a `^prj` with its own live Task Link today
// is desired at Next like any linked task, and the walk still
// continues through it.
#[test]
fn desired_statuses_raise_a_directly_linked_prj() {
    let prj = (PathBuf::from("proj.md"), "prj".to_string());
    let pre = identity("pre");
    let direct = BTreeSet::from([prj.clone()]);
    let edges = BTreeMap::from([(prj.clone(), BTreeSet::from([pre.clone()]))]);
    let task_blocks =
        BTreeMap::from([(prj.clone(), vec!['?']), (pre.clone(), vec![' '])]);
    let project_tasks = BTreeSet::from([prj.clone()]);

    let desired =
        desired_statuses(&direct, &edges, &task_blocks, &project_tasks);

    assert_eq!(desired.get(&prj), Some(&RankedStatus::Next));
    assert_eq!(desired.get(&pre), Some(&RankedStatus::Next));
}

// §13.3: residents inherit `^prj` links as implicit promotion edges;
// residents of unblocked projects and non-residents gain none.
#[test]
fn residents_inherit_prj_links_as_implicit_edges() {
    let proj = format!(
        "{PROJECT_FRONTMATTER}- [?] #task #prj Ship [dependsOn:: t__pre] ^prj\n- [ ] #task Resident ^r1\n"
    );
    let scanned = edge_scan(&[
        ("proj.md", proj.as_str()),
        ("t.md", "- [ ] #task Prerequisite [id:: t__pre] ^pre\n"),
    ]);
    assert_eq!(
        scanned.edges.get(&edge("proj.md", "r1")),
        Some(&BTreeSet::from([edge("t.md", "pre")])),
    );
    assert!(scanned.edges.get(&edge("proj.md", "prj")).is_none());
}

// PDB2: `^prj` reports only its own reasons, never a `project_*`
// reason pointing at itself.
#[test]
fn prj_reports_only_own_reasons() {
    let file = project_file(
        "proj.md",
        "- [?] #task #prj Ship [dependsOn:: t__pre] ^prj\n",
    );
    let context = ProjectContext::for_note(&file);
    assert!(context.recognised);
    let prj = &file.tasks[0];
    let reasons =
        project_block_reasons(&context, &prj.depends_on, date(2026, 7, 16));
    assert!(!reasons.is_empty());
    let effective = effective_block_reasons(
        prj,
        &prj.depends_on,
        &context,
        &reasons,
        &[],
        date(2026, 7, 16),
    );
    assert_eq!(
        effective.iter().map(BlockReason::kind).collect::<Vec<_>>(),
        ["local_dependency"]
    );
}

// A hand-set `[?]` on `^prj` blocks nobody: with no own reasons the
// transition recovers instead of staying Blocked.
#[test]
fn hand_blocked_prj_with_no_reasons_recovers() {
    let file = project_file(
        "proj.md",
        "- [?] #task #prj Ship ^prj\n- [ ] #task Resident ^r1\n",
    );
    let context = ProjectContext::for_note(&file);
    assert!(context.recognised);
    let reasons = project_block_reasons(&context, &[], date(2026, 7, 16));
    assert!(reasons.is_empty());
    let prj = &file.tasks[0];
    let effective = effective_block_reasons(
        prj,
        &[],
        &context,
        &reasons,
        &[],
        date(2026, 7, 16),
    );
    assert!(effective.is_empty());
    assert_eq!(
        task_transition(prj, None, None, false, false, false),
        Transition::Unblock(RankedStatus::Ready)
    );
    // The resident stays Ready too: no project reason exists.
    let resident = &file.tasks[1];
    let resident_effective = effective_block_reasons(
        resident,
        &[],
        &context,
        &reasons,
        &[],
        date(2026, 7, 16),
    );
    assert!(resident_effective.is_empty());
}

// PDB1 / PDT1: a blocked project marks its residents `[?]` and tags
// each open resident exactly once; `^prj` is never tagged.
#[test]
fn blocked_project_marks_and_tags_residents() {
    let file = project_file(
        "proj.md",
        "- [?] #task #prj Ship [dependsOn:: t__pre] ^prj\n- [ ] #task Resident [fresh:: 2026-07-16] ^r1\n",
    );
    let context = ProjectContext::for_note(&file);
    let reasons = project_block_reasons(
        &context,
        &["t__pre".to_string()],
        date(2026, 7, 16),
    );
    assert!(!reasons.is_empty());
    let (mut added, mut stripped, mut written, mut pruned) = empty_reports();
    let resident = &file.tasks[1];
    let effective = effective_block_reasons(
        resident,
        &[],
        &context,
        &reasons,
        &[],
        date(2026, 7, 16),
    );
    assert_eq!(
        effective.iter().map(BlockReason::kind).collect::<Vec<_>>(),
        ["project_dependency"]
    );
    assert_eq!(
        task_transition(resident, None, None, false, true, false),
        Transition::MarkBlocked
    );
    let lines = logical_lines(&file.contents);
    let rewritten = plan_task_line_rewrite(
        &file,
        resident,
        lines[resident.line_index],
        Some('?'),
        &context,
        &reasons,
        &[],
        Transition::MarkBlocked,
        date(2026, 7, 16),
        false,
        &BTreeSet::new(),
        &mut added,
        &mut stripped,
        &mut written,
        &mut pruned,
    )
    .expect("blocked resident gains a rewrite");
    assert_eq!(
        rewritten,
        "- [?] #task Resident [fresh:: 2026-07-16] #prj_is_blocked ^r1"
    );
    assert_eq!(added.len(), 1);
    assert_eq!(added[0].block_id, "r1");
    assert!(stripped.is_empty());
    assert!(written.is_empty());
    assert!(pruned.is_empty());
}

// PDT2 / UB: recovery strips the tag and writes `[unblocked::]` on a
// stamped resident that lands Ready.
#[test]
fn recovery_strips_tag_and_writes_unblocked() {
    let file = project_file(
        "proj.md",
        "- [ ] #task #prj Ship ^prj\n- [?] #task Resident [fresh:: 2026-07-16] #prj_is_blocked ^r1\n",
    );
    let context = ProjectContext::for_note(&file);
    let reasons = ProjectBlockReasons::default();
    let (mut added, mut stripped, mut written, mut pruned) = empty_reports();
    let resident = &file.tasks[1];
    assert_eq!(
        task_transition(resident, None, None, false, false, false),
        Transition::Unblock(RankedStatus::Ready)
    );
    let lines = logical_lines(&file.contents);
    let rewritten = plan_task_line_rewrite(
        &file,
        resident,
        lines[resident.line_index],
        Some(' '),
        &context,
        &reasons,
        &[],
        Transition::Unblock(RankedStatus::Ready),
        date(2026, 7, 16),
        false,
        &BTreeSet::new(),
        &mut added,
        &mut stripped,
        &mut written,
        &mut pruned,
    )
    .expect("recovered resident gains a rewrite");
    assert_eq!(
        rewritten,
        "- [ ] #task Resident [fresh:: 2026-07-16] [unblocked:: 2026-07-16] ^r1"
    );
    assert!(added.is_empty());
    assert_eq!(stripped.len(), 1);
    assert_eq!(written.len(), 1);
    assert_eq!(written[0].date, "2026-07-16");
    assert!(pruned.is_empty());
}

// UB8: a stale `[unblocked:: U]` (`fresh > U`) is pruned from an open
// line with no status change.
#[test]
fn stale_unblocked_field_is_pruned_without_status_change() {
    let file = project_file(
        "proj.md",
        "- [ ] #task #prj Ship ^prj\n- [ ] #task Resident [fresh:: 2026-07-20] [unblocked:: 2026-07-10] ^r1\n",
    );
    let context = ProjectContext::for_note(&file);
    let reasons = ProjectBlockReasons::default();
    let (mut added, mut stripped, mut written, mut pruned) = empty_reports();
    let resident = &file.tasks[1];
    let lines = logical_lines(&file.contents);
    let rewritten = plan_task_line_rewrite(
        &file,
        resident,
        lines[resident.line_index],
        None,
        &context,
        &reasons,
        &[],
        Transition::Unchanged,
        date(2026, 7, 20),
        false,
        &BTreeSet::new(),
        &mut added,
        &mut stripped,
        &mut written,
        &mut pruned,
    )
    .expect("stale unblocked field is pruned");
    assert_eq!(rewritten, "- [ ] #task Resident [fresh:: 2026-07-20] ^r1");
    assert!(added.is_empty() && stripped.is_empty() && written.is_empty());
    assert_eq!(pruned.len(), 1);
    assert_eq!(pruned[0].date, "2026-07-10");
}

// PDT5: a hand-copied tag on an open task outside any blocked project
// is stripped.
#[test]
fn hand_copied_tag_outside_blocked_projects_is_stripped() {
    let settings = test_settings();
    let contents = "- [ ] #task Lone #prj_is_blocked ^lone\n".to_string();
    let file = FileScan {
        path: PathBuf::from("tasks.md"),
        relative_path: PathBuf::from("tasks.md"),
        tasks: parse_tasks(&contents, &settings),
        contents,
    };
    let context = ProjectContext::for_note(&file);
    assert!(!context.recognised);
    let (mut added, mut stripped, mut written, mut pruned) = empty_reports();
    let task = &file.tasks[0];
    let lines = logical_lines(&file.contents);
    let rewritten = plan_task_line_rewrite(
        &file,
        task,
        lines[task.line_index],
        None,
        &context,
        &ProjectBlockReasons::default(),
        &[],
        Transition::Unchanged,
        date(2026, 7, 16),
        false,
        &BTreeSet::new(),
        &mut added,
        &mut stripped,
        &mut written,
        &mut pruned,
    )
    .expect("hand-copied tag is stripped");
    assert_eq!(rewritten, "- [ ] #task Lone ^lone");
    assert!(added.is_empty());
    assert_eq!(stripped.len(), 1);
}

// A misplaced existing tag is rewritten into place and is not counted
// as a tag addition.
#[test]
fn misplaced_existing_tag_is_not_counted_as_added() {
    let file = project_file(
        "proj.md",
        "- [?] #task #prj Ship [dependsOn:: t__pre] ^prj\n- [?] #task Resident #prj_is_blocked [fresh:: 2026-07-16] ^r1\n",
    );
    let context = ProjectContext::for_note(&file);
    let reasons = project_block_reasons(
        &context,
        &["t__pre".to_string()],
        date(2026, 7, 16),
    );
    let (mut added, mut stripped, mut written, mut pruned) = empty_reports();
    let resident = &file.tasks[1];
    let lines = logical_lines(&file.contents);
    let rewritten = plan_task_line_rewrite(
        &file,
        resident,
        lines[resident.line_index],
        None,
        &context,
        &reasons,
        &[],
        Transition::Unchanged,
        date(2026, 7, 16),
        false,
        &BTreeSet::new(),
        &mut added,
        &mut stripped,
        &mut written,
        &mut pruned,
    )
    .expect("misplaced tag is moved");
    assert_eq!(
        rewritten,
        "- [?] #task Resident [fresh:: 2026-07-16] #prj_is_blocked ^r1"
    );
    assert!(added.is_empty(), "moving a tag is not an add: {added:?}");
    assert!(stripped.is_empty());
}

// UB5: `(repeat:: …)` is a recurring skip, matching the shared check.
#[test]
fn parenthesized_repeat_skips_unblocked_write() {
    let file = project_file(
        "proj.md",
        "- [ ] #task #prj Ship ^prj\n- [?] #task Resident [fresh:: 2026-07-16] #prj_is_blocked (repeat:: 1) ^r1\n",
    );
    let context = ProjectContext::for_note(&file);
    let reasons = ProjectBlockReasons::default();
    let (mut added, mut stripped, mut written, mut pruned) = empty_reports();
    let resident = &file.tasks[1];
    let lines = logical_lines(&file.contents);
    let rewritten = plan_task_line_rewrite(
        &file,
        resident,
        lines[resident.line_index],
        Some(' '),
        &context,
        &reasons,
        &[],
        Transition::Unblock(RankedStatus::Ready),
        date(2026, 7, 16),
        false,
        &BTreeSet::new(),
        &mut added,
        &mut stripped,
        &mut written,
        &mut pruned,
    )
    .expect("recurring recovery still rewrites the tag");
    assert_eq!(
        rewritten,
        "- [ ] #task Resident [fresh:: 2026-07-16] (repeat:: 1) ^r1"
    );
    assert!(written.is_empty(), "UB5: no [unblocked::] on recurring");
    assert_eq!(stripped.len(), 1);
}

//! Daily, reference, parse, and status-transition tests.
use super::*;

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
fn rolling_reachability_is_cycle_safe_and_includes_dependencies() {
    let root = identity("root");
    let child = identity("child");
    let leaf = identity("leaf");
    let edges = BTreeMap::from([
        (root.clone(), BTreeSet::from([child.clone()])),
        (child.clone(), BTreeSet::from([leaf.clone()])),
        (leaf.clone(), BTreeSet::from([root.clone()])),
    ]);

    assert_eq!(
        reachable_identities(&BTreeSet::from([root.clone()]), &edges),
        BTreeSet::from([root, child, leaf])
    );
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
    let a_contents =
        "- [ ] #task A ^a\n  ```\nnot a list item\n  ```\n  - ![[B#^b]]\n";
    let b_contents = "- [ ] #task B ^b\n";
    let files = vec![
        FileScan {
            path: PathBuf::from("A.md"),
            relative_path: PathBuf::from("A.md"),
            contents: a_contents.to_string(),
            tasks: parse_tasks(a_contents, &settings),
            note_kind: NoteKind::Other,
        },
        FileScan {
            path: PathBuf::from("B.md"),
            relative_path: PathBuf::from("B.md"),
            contents: b_contents.to_string(),
            tasks: parse_tasks(b_contents, &settings),
            note_kind: NoteKind::Other,
        },
    ];
    let index = NoteIndex::from_paths(
        files.iter().map(|file| file.relative_path.clone()),
    );
    let blocks = task_blocks(&files);
    let edges = dependency_edges(&files, &index, &blocks, &mut Vec::new());
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
        note_kind: NoteKind::Other,
    }];
    let states = task_dependency_states(&files);
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

    let desired = desired_statuses(&direct, &edges, &task_blocks);

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
    );

    assert_eq!(recovery[&blocked_root], RankedStatus::Next);
    assert_eq!(recovery[&working_child], RankedStatus::InProgress);
    assert_eq!(recovery[&blocked_leaf], RankedStatus::InProgress);
}

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use chrono::NaiveDate;

use super::*;
use crate::native::{
    capture_language::DrawerSelector,
    note_tasks,
    task_complete::SuccessorSelection,
    task_locator::TaskLocator,
    task_move::VaultSnapshot,
    vault_links::{LinkResolution, NoteIndex},
};

fn snapshot(notes: &[(&str, &str)]) -> VaultSnapshot {
    VaultSnapshot::from_notes(
        notes
            .iter()
            .map(|(path, text)| (PathBuf::from(path), (*text).to_string())),
    )
}

fn locator(value: &str) -> TaskLocator {
    TaskLocator::parse(value).expect("valid test task locator")
}

fn request(source: &[&str], target: MergeContainer) -> MergeRequest {
    MergeRequest {
        sources: source.iter().map(|value| locator(value)).collect(),
        target,
        label: "merged into".to_string(),
        gesture: MergeGesture::Explicit,
        selection: SuccessorSelection::None,
    }
}

fn prepare(
    snapshot: &VaultSnapshot,
    request: &MergeRequest,
) -> Result<PreparedMerge, MergeError> {
    let settings = note_tasks::read_settings(Path::new("/no-bob-vault"));
    let note_index = NoteIndex::from_paths(snapshot.notes.keys().cloned());
    let catalog = BTreeMap::new();
    let context = MergeContext {
        today: NaiveDate::from_ymd_opt(2026, 11, 3).expect("valid date"),
        daily_path: PathBuf::from("daily/2026-11-03.md"),
        daily_read: DailyNoteReadState::NotRead,
        task_settings: &settings,
        note_index: &note_index,
        drawer_catalog: &catalog,
        relation_policy: MergeRelationPolicy::default(),
        successor_labels: &[],
    };
    prepare_task_merge(snapshot, request, &context)
}

fn plan(
    snapshot: &VaultSnapshot,
    request: &MergeRequest,
) -> Result<MergePlan, MergeError> {
    plan_with_policy(snapshot, request, MergeRelationPolicy::default(), &[])
}

fn plan_with_policy(
    snapshot: &VaultSnapshot,
    request: &MergeRequest,
    relation_policy: MergeRelationPolicy,
    successor_labels: &[crate::native::config::relations::CancelLabel],
) -> Result<MergePlan, MergeError> {
    let settings = note_tasks::read_settings(Path::new("/no-bob-vault"));
    let note_index = NoteIndex::from_paths(snapshot.notes.keys().cloned());
    let catalog = BTreeMap::new();
    let context = MergeContext {
        today: NaiveDate::from_ymd_opt(2026, 11, 3).expect("valid date"),
        daily_path: PathBuf::from("daily/2026-11-03.md"),
        daily_read: DailyNoteReadState::NotRead,
        task_settings: &settings,
        note_index: &note_index,
        drawer_catalog: &catalog,
        relation_policy,
        successor_labels,
    };
    plan_task_merge(snapshot, request, &context)
}

fn post<'a>(plan: &'a PreparedMerge, path: &str) -> &'a str {
    plan.postimages
        .get(Path::new(path))
        .map(String::as_str)
        .unwrap_or_else(|| panic!("missing postimage {path}"))
}

fn block_ids(notes: &BTreeMap<PathBuf, String>) -> BTreeSet<(PathBuf, String)> {
    notes
        .iter()
        .flat_map(|(path, contents)| {
            crate::native::collect_done::block_ids_in_markdown(contents)
                .into_iter()
                .map(|block_id| (path.clone(), block_id))
        })
        .collect()
}

fn authored_child_text(
    contents: &str,
    task: &note_tasks::NoteTask,
    settings: &note_tasks::NoteTaskSettings,
) -> String {
    let lines = crate::native::capture::line_spans(contents);
    let children = crate::native::task_children::classify_direct_children(
        contents, task, settings,
    );
    let end_line = lines
        .iter()
        .position(|line| line.end > task.block_end)
        .unwrap_or(lines.len());
    let mut ranges = Vec::new();
    let mut cursor = task.line_index + 1;
    for (offset, child) in children.iter().enumerate() {
        if !child.kind.is_managed() {
            continue;
        }
        let end = children
            .get(offset + 1)
            .map(|next| next.line_index)
            .unwrap_or(end_line);
        if cursor < child.line_index {
            ranges.push(cursor..child.line_index);
        }
        cursor = cursor.max(end);
    }
    if cursor < end_line {
        ranges.push(cursor..end_line);
    }
    let all_lines: Vec<&str> = contents.split_inclusive('\n').collect();
    ranges
        .into_iter()
        .filter_map(|range| {
            all_lines
                .get(range.start..range.end)
                .map(|slice| slice.concat())
        })
        .collect()
}

fn block_link_targets(
    notes: &BTreeMap<PathBuf, String>,
) -> Vec<(PathBuf, String)> {
    let index = NoteIndex::from_paths(notes.keys().cloned());
    notes
        .iter()
        .flat_map(|(owner, contents)| {
            let mut targets = Vec::new();
            let mut remaining = contents.as_str();
            while let Some(start) = remaining.find("[[") {
                remaining = &remaining[start + 2..];
                let Some(end) = remaining.find("]]") else {
                    break;
                };
                let link = remaining[..end]
                    .split_once('|')
                    .map(|(reference, _)| reference)
                    .unwrap_or(&remaining[..end]);
                remaining = &remaining[end + 2..];
                let Some((target, block_id)) = link.split_once("#^") else {
                    continue;
                };
                let path = match index.resolve_detailed(Some(owner), target) {
                    LinkResolution::Found(path) => path,
                    LinkResolution::Ambiguous | LinkResolution::Missing => {
                        continue;
                    }
                };
                targets.push((path, block_id.to_string()));
            }
            targets
        })
        .collect()
}

fn assert_planner_invariants(
    case: &str,
    before: &VaultSnapshot,
    request: &MergeRequest,
) {
    let settings = note_tasks::read_settings(Path::new("/no-bob-vault"));
    let note_index = NoteIndex::from_paths(before.notes.keys().cloned());
    let catalog = BTreeMap::new();
    let context = MergeContext {
        today: NaiveDate::from_ymd_opt(2026, 11, 3).expect("valid date"),
        daily_path: PathBuf::from("daily/2026-11-03.md"),
        daily_read: DailyNoteReadState::NotRead,
        task_settings: &settings,
        note_index: &note_index,
        drawer_catalog: &catalog,
        relation_policy: MergeRelationPolicy::default(),
        successor_labels: &[],
    };
    let first = plan_task_merge(before, request, &context)
        .unwrap_or_else(|error| panic!("{case}: {error:?}"));
    let repeated = plan_task_merge(before, request, &context)
        .unwrap_or_else(|error| panic!("{case}: {error:?}"));
    assert_eq!(first, repeated, "{case}: planning must be deterministic");

    let write_set = first.write_order.iter().cloned().collect::<BTreeSet<_>>();
    assert_eq!(
        write_set.len(),
        first.write_order.len(),
        "{case}: write path duplicated"
    );
    assert_eq!(
        write_set,
        first.postimages.keys().cloned().collect(),
        "{case}: write order must cover every postimage"
    );
    assert!(
        first
            .read_paths
            .is_superset(&before.notes.keys().cloned().collect()),
        "{case}: read set omitted a snapshot note"
    );
    assert!(
        first.read_paths.contains(&context.daily_path),
        "{case}: read set omitted the daily note"
    );

    let source_tasks = request
        .sources
        .iter()
        .map(|locator| {
            containers::resolve_task(before, locator, &context)
                .unwrap_or_else(|error| panic!("{case}: {error:?}"))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        first.records.len(),
        source_tasks.len(),
        "{case}: record count"
    );
    for (record, source) in first.records.iter().zip(&source_tasks) {
        assert_eq!(record.source.path, source.path, "{case}: record order");
        assert_eq!(
            record.source.block_id,
            source.task.block_id.clone().unwrap_or_default(),
            "{case}: record identity/order"
        );
        if record.outcome == MergeOutcome::Merged {
            let moved = if record.moved_notes == 0 {
                "title only".to_string()
            } else if record.moved_notes == 1 {
                "1 note copied".to_string()
            } else {
                format!("{} notes copied", record.moved_notes)
            };
            assert_eq!(
                record.message,
                format!(
                    "⊘ {}  {}:{} · {} → Cancelled · {moved}",
                    record.source.title,
                    record.source.path.display(),
                    record.source.line,
                    source.task.status_name,
                ),
                "{case}: canonical source copy",
            );
        }
    }
    serde_json::to_value(&first)
        .unwrap_or_else(|error| panic!("{case}: plan serialization: {error}"));

    let target_path = match &request.target {
        MergeContainer::Task(locator) | MergeContainer::Drawer(locator, _) => {
            containers::resolve_task(before, locator, &context)
                .unwrap_or_else(|error| panic!("{case}: {error:?}"))
                .path
        }
        MergeContainer::Section { note, .. } => {
            match note_index.resolve_detailed(None, note) {
                LinkResolution::Found(path) => path,
                LinkResolution::Ambiguous | LinkResolution::Missing => {
                    panic!("{case}: section note does not resolve: {note}")
                }
            }
        }
    };
    if let Some(target_position) = first
        .write_order
        .iter()
        .position(|path| path == &target_path)
    {
        let dependent_paths = first
            .records
            .iter()
            .flat_map(|record| record.dependents_repointed.iter())
            .filter_map(|dependent| {
                dependent
                    .split_once('#')
                    .map(|(path, _)| PathBuf::from(path))
            })
            .collect::<BTreeSet<_>>();
        for dependent_path in dependent_paths {
            if dependent_path == target_path {
                continue;
            }
            if let Some(position) = first
                .write_order
                .iter()
                .position(|path| path == &dependent_path)
            {
                assert!(
                    target_position < position,
                    "{case}: target must precede dependents"
                );
            }
        }
    }

    let mut after = before.notes.clone();
    after.extend(first.postimages.clone());
    let before_blocks = block_ids(&before.notes);
    let after_blocks = block_ids(&after);
    let source_roots = source_tasks
        .iter()
        .map(|source| {
            (
                source.path.clone(),
                source.task.block_id.clone().unwrap_or_default(),
            )
        })
        .collect::<BTreeSet<_>>();
    let source_descendants = source_tasks
        .iter()
        .map(|source| {
            let contents = before
                .notes
                .get(&source.path)
                .expect("source note in snapshot");
            let scan = note_tasks::scan(contents, &settings);
            let task = scan
                .tasks()
                .iter()
                .find(|task| {
                    task.block_id.as_deref() == source.task.block_id.as_deref()
                })
                .expect("source task in note scan");
            let line_start = contents
                .split_inclusive('\n')
                .take(task.line_index)
                .map(str::len)
                .sum::<usize>();
            let fragment = &contents[line_start..task.block_end];
            let mut ids =
                crate::native::collect_done::block_ids_in_markdown(fragment)
                    .into_iter()
                    .collect::<BTreeSet<_>>();
            if let Some(root) = &source.task.block_id {
                ids.remove(root);
            }
            (source.path.clone(), ids)
        })
        .collect::<Vec<_>>();
    let prior_targets = block_link_targets(&before.notes)
        .into_iter()
        .filter(|target| before_blocks.contains(target))
        .collect::<BTreeSet<_>>();
    for (path, block_id) in prior_targets {
        if source_roots.contains(&(path.clone(), block_id.clone())) {
            assert!(
                after_blocks.contains(&(path.clone(), block_id.clone())),
                "{case}: source tombstone identity disappeared: {}#^{}",
                path.display(),
                block_id,
            );
            continue;
        }
        if source_descendants.iter().any(|(source_path, ids)| {
            source_path == &path && ids.contains(&block_id)
        }) {
            assert!(
                after_blocks.contains(&(path.clone(), block_id.clone())),
                "{case}: source descendant identity disappeared: {}#^{}",
                path.display(),
                block_id,
            );
            continue;
        }
        assert!(
            after_blocks.contains(&(path.clone(), block_id.clone())),
            "{case}: previously linked identity disappeared: {}#^{}",
            path.display(),
            block_id,
        );
    }
    assert!(
        first.renamed_block_ids.is_empty(),
        "{case}: copies never rename anchors"
    );
    for source in &source_tasks {
        if !source.task.status_type.is_open() {
            continue;
        }
        let Some(before_text) = before.notes.get(&source.path) else {
            continue;
        };
        let Some(after_text) = after.get(&source.path) else {
            panic!(
                "{case}: source note missing after merge: {}",
                source.path.display()
            );
        };
        let after_scan = note_tasks::scan(after_text, &settings);
        let after_task = after_scan
            .tasks()
            .iter()
            .find(|task| task.block_id == source.task.block_id)
            .unwrap_or_else(|| {
                panic!(
                    "{case}: source ^{} missing after merge",
                    source.task.block_id.as_deref().unwrap_or("?")
                )
            });
        assert_eq!(
            after_task.status_symbol,
            '-',
            "{case}: source ^{} was not cancelled",
            source.task.block_id.as_deref().unwrap_or("?")
        );
        assert_eq!(
            authored_child_text(before_text, &source.task, &settings),
            authored_child_text(after_text, after_task, &settings),
            "{case}: source authored body changed"
        );
    }
    for target in block_link_targets(&after) {
        assert!(
            after_blocks.contains(&target),
            "{case}: postimage block link is unresolved: {}#^{}",
            target.0.display(),
            target.1,
        );
    }

    let reconciled =
        crate::native::task_status_hooks::reconcile_merge_snapshot_for_tests(
            &after, &settings,
        );
    assert_eq!(reconciled, after, "{case}: production reconciliation drift");

    let after_snapshot = VaultSnapshot::from_notes(after);
    let rerun = plan_task_merge(&after_snapshot, request, &context)
        .unwrap_or_else(|error| panic!("{case}: rerun: {error:?}"));
    assert!(
        rerun.postimages.is_empty(),
        "{case}: rerun should be write-free"
    );
    assert!(
        rerun
            .records
            .iter()
            .all(|record| record.outcome != MergeOutcome::Merged),
        "{case}: rerun should not report a new merge",
    );
}

#[test]
fn mg1_title_only_source_is_prepared_as_one_plain_child() {
    let snapshot = snapshot(&[
        ("src.md", "- [ ] #task Investigate retries ^src\n"),
        ("dest.md", "- [ ] #task Stabilize API ^dest\n"),
    ]);
    let plan = prepare(
        &snapshot,
        &request(&["src:src"], MergeContainer::Task(locator("dest:dest"))),
    )
    .expect("merge payload");
    assert_eq!(
        post(&plan, "dest.md"),
        "- [ ] #task Stabilize API ^dest\n\t- Investigate retries\n"
    );
    assert_eq!(plan.sources[0].disposition, SourceDisposition::Open);
    assert_eq!(plan.sources[0].moved_notes, 0);
}

#[test]
fn mg2_nested_drawers_travel_under_the_source_title_without_flattening() {
    let snapshot = snapshot(&[
        (
            "src.md",
            "- [ ] #task Source ^src\n  - REQUIREMENTS\n    - Authored note\n",
        ),
        (
            "dest.md",
            "- [ ] #task Target ^target\n\t- REQUIREMENTS\n\t\t- Existing\n",
        ),
    ]);
    let plan = prepare(
        &snapshot,
        &request(&["src:src"], MergeContainer::Task(locator("dest:target"))),
    )
    .expect("merge payload");
    let destination = post(&plan, "dest.md");
    assert!(destination
        .contains("\t- Source\n\t\t- REQUIREMENTS\n\t\t\t- Authored note\n"));
    assert!(destination.contains("\t- REQUIREMENTS\n\t\t- Existing\n"));
}

#[test]
fn mg3_daily_note_link_stays_on_source_and_copy_has_no_anchors() {
    let snapshot = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\n\t- Note ^detail\n"),
        (
            "dest.md",
            "- [ ] #task Target ^target\n\t- Existing ^detail\n",
        ),
        ("daily.md", "See [[src#^detail]]\n"),
    ]);
    let plan = prepare(
        &snapshot,
        &request(&["src:src"], MergeContainer::Task(locator("dest:target"))),
    )
    .expect("merge payload");
    let destination = post(&plan, "dest.md");
    assert!(
        destination.contains("\t- Source\n\t\t- Note\n"),
        "{destination}"
    );
    assert!(
        !destination.contains("- Note ^"),
        "copy carries no descendant anchors: {destination}"
    );
    assert!(
        !plan.postimages.contains_key(Path::new("daily.md")),
        "daily-note links stay on S and are not rewritten"
    );
    assert!(plan.renamed_block_ids.is_empty());
    assert_eq!(
        snapshot.notes[Path::new("src.md")].as_str(),
        "- [ ] #task Source ^src\n\t- Note ^detail\n"
    );
    assert_eq!(
        snapshot.notes[Path::new("daily.md")].as_str(),
        "See [[src#^detail]]\n"
    );
}

#[test]
fn colliding_descendant_ids_stay_on_each_source_note() {
    let snapshot = snapshot(&[
        ("first.md", "- [ ] #task First ^first\n\t- Note ^detail\n"),
        (
            "second.md",
            "- [ ] #task Second ^second\n\t- Note ^detail\n",
        ),
        (
            "dest.md",
            "- [ ] #task Target ^target\n\t- Existing ^detail\n",
        ),
        ("daily.md", "[[first#^detail]] and [[second#^detail]]\n"),
    ]);
    let plan = prepare(
        &snapshot,
        &request(
            &["first:first", "second:second"],
            MergeContainer::Task(locator("dest:target")),
        ),
    )
    .expect("merge payloads");

    let destination = post(&plan, "dest.md");
    assert_eq!(
        destination.matches("\t\t- Note\n").count(),
        2,
        "{destination}"
    );
    assert!(
        !destination.contains("- Note ^"),
        "copies carry no descendant anchors: {destination}"
    );
    assert!(
        !plan.postimages.contains_key(Path::new("daily.md")),
        "inbound links stay on each source and are not rewritten"
    );
    assert!(plan.renamed_block_ids.is_empty());
}

#[test]
fn intra_copy_same_note_links_name_the_source_note() {
    let snapshot = snapshot(&[
        (
            "src.md",
            concat!(
                "- [ ] #task Source ^src\n",
                "\t- See [[#^detail]] and [[#^src]]\n",
                "\t- Keep `[[#^detail]]` and [[other#^related]]\n",
                "\t- Note ^detail\n",
                "\t  ```text\n",
                "\t  [[#^detail]]\n",
                "\t  ```\n",
            ),
        ),
        ("dest.md", "- [ ] #task Target ^target\n"),
        ("other.md", "- [ ] #task Related ^related\n"),
    ]);
    let plan = prepare(
        &snapshot,
        &request(&["src:src"], MergeContainer::Task(locator("dest:target"))),
    )
    .expect("merge payload");
    let destination = post(&plan, "dest.md");
    assert!(
        destination.contains("See [[src#^detail]] and [[src#^src]]"),
        "{destination}"
    );
    assert!(
        destination.contains("Keep `[[#^detail]]` and [[other#^related]]"),
        "{destination}"
    );
    assert!(
        destination.contains("```text\n\t\t  [[#^detail]]\n"),
        "{destination}"
    );
    assert!(
        !destination.contains("^detail\n"),
        "copy strips live anchors: {destination}"
    );
    assert_eq!(plan.repaired_files.len(), 1);
    assert_eq!(plan.repaired_files[0].path, PathBuf::from("dest.md"));
    assert!(plan.repaired_files[0].link_count >= 2, "{plan:?}");
}

#[test]
fn mg4_tables_and_fences_are_rebased_as_authored_payload() {
    let snapshot = snapshot(&[
        (
            "src.md",
            "- [ ] #task Source ^src\n  - Details\n    | A | B |\n    | - | - |\n    ```text\n    - not a task\n    ```\n",
        ),
        ("dest.md", "- [ ] #task Target ^target\n"),
    ]);
    let plan = prepare(
        &snapshot,
        &request(&["src:src"], MergeContainer::Task(locator("dest:target"))),
    )
    .expect("merge payload");
    let destination = post(&plan, "dest.md");
    assert!(destination
        .contains("\t\t- Details\n\t\t\t| A | B |\n\t\t\t| - | - |\n"));
    assert!(
        destination.contains("\t\t  ```text\n\t\t  - not a task\n\t\t  ```\n")
    );
    assert_eq!(plan.sources[0].moved_notes, 1);
}

#[test]
fn mg14_section_slug_near_duplicate_refuses() {
    let snapshot = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\n"),
        (
            "areas/work.md",
            "---\ntype: [[area]]\n---\n## Future Work\n- Existing\n",
        ),
    ]);
    let error = prepare(
        &snapshot,
        &request(
            &["src:src"],
            MergeContainer::Section {
                note: "areas/work".to_string(),
                selector: DrawerSelector {
                    text: "future".to_string(),
                    exact: false,
                },
            },
        ),
    )
    .expect_err("strict prefix must refuse");
    assert_eq!(error.code, "merge_section_near_duplicate");
}

#[test]
fn mg15_section_slug_title_cases_and_exact_literal_preserves_text() {
    let snapshot = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\n"),
        ("areas/work.md", "---\ntype: [[area]]\n---\n"),
    ]);
    let slug = prepare(
        &snapshot,
        &request(
            &["src:src"],
            MergeContainer::Section {
                note: "areas/work".to_string(),
                selector: DrawerSelector {
                    text: "future-work".to_string(),
                    exact: false,
                },
            },
        ),
    )
    .expect("new section");
    assert_eq!(
        post(&slug, "areas/work.md"),
        "---\ntype: [[area]]\n---\n\n## Future Work\n\n- Source\n"
    );

    let exact = prepare(
        &snapshot,
        &request(
            &["src:src"],
            MergeContainer::Section {
                note: "areas/work".to_string(),
                selector: DrawerSelector {
                    text: "Exact title".to_string(),
                    exact: true,
                },
            },
        ),
    )
    .expect("exact title");
    assert!(
        post(&exact, "areas/work.md").contains("## Exact title\n\n- Source\n")
    );
}

#[test]
fn section_resolver_uses_existing_headings_and_refuses_structural_titles() {
    let snapshot = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\n"),
        (
            "areas/work.md",
            "---\ntype: [[area]]\n---\n## Future Work\n- Existing\n## Tasks\n",
        ),
    ]);
    let existing = prepare(
        &snapshot,
        &request(
            &["src:src"],
            MergeContainer::Section {
                note: "areas/work".to_string(),
                selector: DrawerSelector {
                    text: "future-work".to_string(),
                    exact: false,
                },
            },
        ),
    )
    .expect("existing heading by slug");
    assert!(post(&existing, "areas/work.md")
        .contains("## Future Work\n- Existing\n- Source\n"));

    let reserved = prepare(
        &snapshot,
        &request(
            &["src:src"],
            MergeContainer::Section {
                note: "areas/work".to_string(),
                selector: DrawerSelector {
                    text: "Tasks".to_string(),
                    exact: false,
                },
            },
        ),
    )
    .expect_err("Tasks is structural");
    assert_eq!(reserved.code, "merge_section_reserved");
}

#[test]
fn mg20_source_groups_keep_typed_order_after_existing_entries() {
    let snapshot = snapshot(&[
        ("first.md", "- [ ] #task First ^first\n"),
        ("second.md", "- [ ] #task Second ^second\n"),
        ("dest.md", "- [ ] #task Target ^target\n\t- Existing\n"),
    ]);
    let plan = prepare(
        &snapshot,
        &request(
            &["second:second", "first:first"],
            MergeContainer::Task(locator("dest:target")),
        ),
    )
    .expect("merge payload");
    let destination = post(&plan, "dest.md");
    assert!(
        destination.find("\t- Existing").unwrap()
            < destination.find("\t- Second").unwrap()
    );
    assert!(
        destination.find("\t- Second").unwrap()
            < destination.find("\t- First").unwrap()
    );
    assert_eq!(
        plan.payloads
            .iter()
            .map(|payload| payload.source_index)
            .collect::<Vec<_>>(),
        [0, 1]
    );
}

#[test]
fn task_drawers_and_project_h2_drawers_resolve_for_merge() {
    let selector = |text: &str| DrawerSelector {
        text: text.to_string(),
        exact: false,
    };
    let source = ("src.md", "- [ ] #task Source ^src\n");

    let existing = snapshot(&[
        source,
        (
            "dest.md",
            "- [ ] #task Target ^target\n\t- REQUIREMENTS\n\t\t- Existing\n",
        ),
    ]);
    let plan = prepare(
        &existing,
        &request(
            &["src:src"],
            MergeContainer::Drawer(
                locator("dest:target"),
                selector("requirements"),
            ),
        ),
    )
    .expect("existing task drawer");
    assert!(post(&plan, "dest.md")
        .contains("\t- REQUIREMENTS\n\t\t- Existing\n\t\t- Source\n"));

    let missing =
        snapshot(&[source, ("dest.md", "- [ ] #task Target ^target\n")]);
    let plan = prepare(
        &missing,
        &request(
            &["src:src"],
            MergeContainer::Drawer(
                locator("dest:target"),
                selector("requirements"),
            ),
        ),
    )
    .expect("create task drawer");
    assert!(post(&plan, "dest.md").contains("\t- REQUIREMENTS\n\t\t- Source\n"));

    let project = snapshot(&[
        source,
        (
            "project.md",
            "---\ntype: [[project]]\n---\n- [ ] #task Project ^prj\n## Future Work\n- Existing\n",
        ),
    ]);
    let plan = prepare(
        &project,
        &request(
            &["src:src"],
            MergeContainer::Drawer(
                locator("project:prj"),
                selector("future-work"),
            ),
        ),
    )
    .expect("project H2 drawer");
    assert!(post(&plan, "project.md")
        .contains("## Future Work\n- Existing\n- Source\n"));
}

#[test]
fn same_note_destinations_and_nested_sources_keep_originals_and_receive_copies()
{
    for contents in [
        "- [ ] #task Target ^target\n\t- Existing\n- [ ] #task Source ^source\n\t- Note\n",
        "- [ ] #task Source ^source\n\t- Note\n- [ ] #task Target ^target\n\t- Existing\n",
    ] {
        let snapshot = snapshot(&[("same.md", contents)]);
        let plan = prepare(
            &snapshot,
            &request(
                &["same:source"],
                MergeContainer::Task(locator("same:target")),
            ),
        )
        .expect("same-note merge");
        let postimage = post(&plan, "same.md");
        let target = postimage.find("- [ ] #task Target ^target").unwrap();
        let group = postimage.find("\t- Source\n").unwrap();
        let source = postimage.find("- [ ] #task Source ^source").unwrap();
        let target_was_before_source = contents
            .find("- [ ] #task Target ^target")
            .unwrap()
            < contents.find("- [ ] #task Source ^source").unwrap();
        if target_was_before_source {
            assert!(target < group && group < source, "{postimage}");
        } else {
            assert!(source < target && target < group, "{postimage}");
        }
        assert!(postimage.contains("\t\t- Note\n"), "{postimage}");
    }

    let nested = snapshot(&[(
        "same.md",
        "- [ ] #task Parent ^parent\n  - [ ] #task Source ^source\n    - Note\n- [ ] #task Target ^target\n",
    )]);
    let plan = prepare(
        &nested,
        &request(
            &["same:source"],
            MergeContainer::Task(locator("same:target")),
        ),
    )
    .expect("nested source is eligible");
    let postimage = post(&plan, "same.md");
    assert_eq!(
        postimage,
        "- [ ] #task Parent ^parent\n  - [ ] #task Source ^source\n    - Note\n- [ ] #task Target ^target\n  - Source\n    - Note\n"
    );
}

#[test]
fn authored_payload_moves_while_managed_history_stays_on_source() {
    let source = concat!(
        "- [ ] #task Source ^src\n",
        "\t- Authored note\n",
        "\t- 🪢 **LINKS:** [[other#^related]]\n",
        "\t- 🛠️ **WORK LOG**\n",
        "\t\t- *2026-11-02* · 25m\n",
        "\t- ❌ **CANCEL LOG**\n",
        "\t\t- *2026-11-01* — earlier reason\n",
    );
    let snapshot = snapshot(&[
        ("src.md", source),
        ("dest.md", "- [ ] #task Target ^target\n"),
    ]);
    let plan = prepare(
        &snapshot,
        &request(&["src:src"], MergeContainer::Task(locator("dest:target"))),
    )
    .expect("managed children remain on source");

    assert!(post(&plan, "dest.md").contains("\t\t- Authored note\n"));
    assert!(
        !plan.postimages.contains_key(Path::new("src.md")),
        "prepare copies notes and leaves the source note uncut"
    );
    let source_note = snapshot.notes[Path::new("src.md")].as_str();
    assert!(source_note.contains("Authored note"));
    assert!(source_note.contains("🪢 **LINKS:** [[other#^related]]"));
    assert!(source_note.contains("🛠️ **WORK LOG**"));
    assert!(source_note.contains("❌ **CANCEL LOG**"));
    assert!(source_note.contains("earlier reason"));
}

#[test]
fn payload_moves_unwrapped_authored_bytes_around_managed_logs() {
    let paragraph_only = snapshot(&[
        (
            "src.md",
            "- [ ] #task Source ^src\n  A paragraph without a wrapper bullet.\n",
        ),
        ("dest.md", "- [ ] #task Target ^target\n"),
    ]);
    let plan = prepare(
        &paragraph_only,
        &request(&["src:src"], MergeContainer::Task(locator("dest:target"))),
    )
    .expect("paragraph-only payload");
    assert!(post(&plan, "dest.md")
        .contains("\t\tA paragraph without a wrapper bullet."));
    assert!(
        !plan.postimages.contains_key(Path::new("src.md")),
        "prepare copies the paragraph and leaves the source uncut"
    );
    assert!(
        paragraph_only.notes[Path::new("src.md")].contains("paragraph without")
    );
    assert_eq!(plan.sources[0].moved_notes, 1);

    let source = concat!(
        "- [ ] #task Source ^src\n",
        "  Prose before the first child.\n",
        "  | Name | Value |\n",
        "  | - | - |\n",
        "  ```text\n",
        "  fenced - [ ] #task example ^fake\n",
        "  ```\n",
        "  - Authored before the logs\n",
        "  - 🪢 **LINKS:** [[other#^related]]\n",
        "  - ❌ **CANCEL LOG**\n",
        "    - *2026-11-02* — earlier reason\n",
        "  - Authored after the logs\n",
    );
    let snapshot = snapshot(&[
        ("src.md", source),
        ("dest.md", "- [ ] #task Target ^target\n"),
        ("other.md", "- [ ] #task Related ^related\n"),
    ]);
    let plan = prepare(
        &snapshot,
        &request(&["src:src"], MergeContainer::Task(locator("dest:target"))),
    )
    .expect("all task-owned authored ranges copy");
    let destination = post(&plan, "dest.md");
    for copied in [
        "Prose before the first child.",
        "| Name | Value |",
        "fenced - [ ] #task example ^fake",
        "Authored before the logs",
        "Authored after the logs",
    ] {
        assert_eq!(destination.matches(copied).count(), 1, "{copied}");
    }
    assert!(
        !plan.postimages.contains_key(Path::new("src.md")),
        "prepare copies authored bytes and leaves the source uncut"
    );
    let retained = snapshot.notes[Path::new("src.md")].as_str();
    assert!(retained.contains("Prose before the first child"));
    assert!(retained.contains("Authored before the logs"));
    assert!(retained.contains("Authored after the logs"));
    assert!(retained.contains("🪢 **LINKS:** [[other#^related]]"));
    assert!(retained.contains("❌ **CANCEL LOG**"));
    assert!(retained.contains("earlier reason"));
}

#[test]
fn payload_cut_preserves_crlf_and_missing_final_newline() {
    let snapshot = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\r\n  Authored CRLF note"),
        ("dest.md", "- [ ] #task Target ^target\r\n"),
    ]);
    let plan = prepare(
        &snapshot,
        &request(&["src:src"], MergeContainer::Task(locator("dest:target"))),
    )
    .expect("CRLF payload without a final newline");
    let destination = post(&plan, "dest.md");
    assert!(destination.contains("\r\n\t\tAuthored CRLF note"));
    assert!(destination.match_indices('\n').all(|(index, _)| {
        index > 0 && destination.as_bytes()[index - 1] == b'\r'
    }));
    assert!(
        !plan.postimages.contains_key(Path::new("src.md")),
        "prepare copies CRLF notes and leaves the source uncut"
    );
    let source = snapshot.notes[Path::new("src.md")].as_str();
    assert!(source.starts_with("- [ ] #task Source ^src\r\n"));
}

#[test]
fn structural_refusals_cover_nested_open_tasks_and_destination_relationships() {
    let nested = snapshot(&[
        (
            "src.md",
            "- [ ] #task Source ^src\n  - REQUIREMENTS\n    - [ ] #task Open below drawer ^child\n",
        ),
        ("dest.md", "- [ ] #task Target ^target\n"),
    ]);
    assert_eq!(
        prepare(
            &nested,
            &request(
                &["src:src"],
                MergeContainer::Task(locator("dest:target"))
            ),
        )
        .expect_err("open nested task")
        .code,
        "merge_source_open_subtasks"
    );

    let target_inside = snapshot(&[(
        "same.md",
        "- [ ] #task Source ^src\n  - [ ] #task Target ^target\n",
    )]);
    assert_eq!(
        prepare(
            &target_inside,
            &request(
                &["same:src"],
                MergeContainer::Task(locator("same:target"))
            ),
        )
        .expect_err("target nested in source")
        .code,
        "merge_target_inside_source"
    );

    assert_eq!(
        prepare(
            &nested,
            &request(&["src:src"], MergeContainer::Task(locator("src:src"))),
        )
        .expect_err("target also source")
        .code,
        "merge_target_is_source"
    );
}

#[test]
fn source_identity_and_task_structure_refusals_are_specific() {
    let repeated = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\n"),
        ("dest.md", "- [ ] #task Target ^target\n"),
    ]);
    assert_eq!(
        prepare(
            &repeated,
            &request(
                &["src:src", "src:src"],
                MergeContainer::Task(locator("dest:target")),
            ),
        )
        .expect_err("duplicate resolved source")
        .code,
        "merge_duplicate_source"
    );

    let overlap = snapshot(&[
        (
            "same.md",
            "- [ ] #task Parent ^parent\n  - [ ] #task Child ^child\n",
        ),
        ("dest.md", "- [ ] #task Target ^target\n"),
    ]);
    assert_eq!(
        prepare(
            &overlap,
            &request(
                &["same:parent", "same:child"],
                MergeContainer::Task(locator("dest:target")),
            ),
        )
        .expect_err("overlapping source blocks")
        .code,
        "merge_source_overlap"
    );

    for (source, code) in [
        (
            "- [ ] #task Repeats [repeat:: daily] ^source\n",
            "merge_source_recurring",
        ),
        ("- [ ] #task Project ^prj\n", "merge_source_project"),
    ] {
        let snapshot = snapshot(&[
            ("src.md", source),
            ("dest.md", "- [ ] #task Target ^target\n"),
        ]);
        assert_eq!(
            prepare(
                &snapshot,
                &request(
                    &[if code == "merge_source_project" {
                        "src:prj"
                    } else {
                        "src:source"
                    }],
                    MergeContainer::Task(locator("dest:target")),
                ),
            )
            .expect_err("ineligible source")
            .code,
            code
        );
    }

    let closed_target = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\n"),
        ("dest.md", "- [x] #task Closed ^target\n"),
    ]);
    assert_eq!(
        prepare(
            &closed_target,
            &request(
                &["src:src"],
                MergeContainer::Task(locator("dest:target"))
            ),
        )
        .expect_err("closed destination")
        .code,
        "merge_target_closed"
    );

    let mixed = snapshot(&[
        ("closed.md", "- [x] #task Closed source ^closed\n"),
        ("open.md", "- [ ] #task Open source ^open\n"),
        ("dest.md", "- [ ] #task Target ^target\n"),
    ]);
    let plan = prepare(
        &mixed,
        &request(
            &["closed:closed", "open:open"],
            MergeContainer::Task(locator("dest:target")),
        ),
    )
    .expect("retain source disposition for retirement");
    assert_eq!(plan.sources[0].disposition, SourceDisposition::Closed);
    assert_eq!(plan.sources[1].disposition, SourceDisposition::Open);
}

// ---- retirement (phase 4.1.2) ----

fn retire(
    snapshot: &VaultSnapshot,
    request: &MergeRequest,
) -> Result<(BTreeMap<PathBuf, String>, Vec<RetirementReports>), MergeError> {
    retire_with_successor_labels(snapshot, request, &[])
}

fn retire_with_successor_labels(
    snapshot: &VaultSnapshot,
    request: &MergeRequest,
    successor_labels: &[crate::native::config::relations::CancelLabel],
) -> Result<(BTreeMap<PathBuf, String>, Vec<RetirementReports>), MergeError> {
    retire_with_policies(
        snapshot,
        request,
        MergeRelationPolicy::default(),
        successor_labels,
    )
}

fn retire_with_policies(
    snapshot: &VaultSnapshot,
    request: &MergeRequest,
    relation_policy: MergeRelationPolicy,
    successor_labels: &[crate::native::config::relations::CancelLabel],
) -> Result<(BTreeMap<PathBuf, String>, Vec<RetirementReports>), MergeError> {
    let settings = note_tasks::read_settings(Path::new("/no-bob-vault"));
    let note_index = NoteIndex::from_paths(snapshot.notes.keys().cloned());
    let catalog = BTreeMap::new();
    let context = MergeContext {
        today: NaiveDate::from_ymd_opt(2026, 11, 3).expect("valid date"),
        daily_path: PathBuf::from("daily/2026-11-03.md"),
        daily_read: DailyNoteReadState::NotRead,
        task_settings: &settings,
        note_index: &note_index,
        drawer_catalog: &catalog,
        relation_policy,
        successor_labels,
    };
    let prepared = prepare_task_merge(snapshot, request, &context)?;
    let target = match &request.target {
        MergeContainer::Task(locator) | MergeContainer::Drawer(locator, _) => {
            let located = crate::native::task_locator::resolve_task(
                &snapshot.notes,
                &format!("{}:{}", locator.note, locator.block_id),
                &settings,
            )
            .map_err(|error| MergeError::new(error.code, error.message))?;
            Some((located.path, located.task.block_id.unwrap_or_default()))
        }
        MergeContainer::Section { .. } => None,
    };
    apply_retirement(snapshot, &prepared, request, target, &context)
}

fn staged_text(staged: &BTreeMap<PathBuf, String>, path: &str) -> String {
    staged
        .get(Path::new(path))
        .cloned()
        .unwrap_or_else(|| panic!("missing staged note {path}"))
}

fn classify(
    snapshot: &VaultSnapshot,
    source: &str,
    target: MergeContainer,
    label: &str,
    gesture: MergeGesture,
) -> (SourceOutcome, SourceDisposition) {
    let settings = note_tasks::read_settings(Path::new("/no-bob-vault"));
    let note_index = NoteIndex::from_paths(snapshot.notes.keys().cloned());
    let catalog = BTreeMap::new();
    let context = MergeContext {
        today: NaiveDate::from_ymd_opt(2026, 11, 3).expect("valid date"),
        daily_path: PathBuf::from("daily/2026-11-03.md"),
        daily_read: DailyNoteReadState::NotRead,
        task_settings: &settings,
        note_index: &note_index,
        drawer_catalog: &catalog,
        relation_policy: MergeRelationPolicy::default(),
        successor_labels: &[],
    };
    let request = MergeRequest {
        sources: vec![locator(source)],
        target,
        label: label.to_string(),
        gesture,
        selection: SuccessorSelection::None,
    };
    classify_source(
        snapshot,
        &request.sources[0],
        &request.target,
        label,
        &context,
    )
    .expect("classification")
}

#[test]
fn mg5_open_dependents_repoint_with_mirrors_and_provenance() {
    let snapshot = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\n"),
        ("dest.md", "- [ ] #task Target ^dst\n"),
        (
            "dep.md",
            "- [ ] #task Dependent ^dep\n\t- ⛓️ **DEPENDS ON:** [[src#^src]]\n",
        ),
    ]);
    let (staged, reports) = retire(
        &snapshot,
        &request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
    )
    .expect("retire");
    let dependent = staged_text(&staged, "dep.md");
    assert!(
        dependent.contains("⛓️ **DEPENDS ON:** [[dest#^dst]]"),
        "{dependent}"
    );
    assert!(!dependent.contains("src#^src"), "{dependent}");
    assert!(!dependent.contains("[unblocked::]"), "{dependent}");
    let destination = staged_text(&staged, "dest.md");
    assert!(
        destination.contains("🧱 **BLOCKS:** [[dep#^dep]]"),
        "{destination}"
    );
    assert!(
        destination.contains("LINKED FROM")
            && destination.contains("(merged into)"),
        "{destination}"
    );
    let source = staged_text(&staged, "src.md");
    assert!(source.starts_with("- [-]"), "{source}");
    assert!(source.contains("merged into [[dest#^dst]]"), "{source}");
    assert_eq!(reports.len(), 1);
    assert_eq!(
        reports[0].dependents_repointed,
        vec!["dep.md#^dep".to_string()]
    );
}

#[test]
fn planner_validates_duplicates_and_source_structure_before_retry_filtering() {
    let duplicate = snapshot(&[
        (
            "src.md",
            "- [-] #task Retired ^src\n\t- 🪢 **LINKS:** merged into [[dest#^target]]\n",
        ),
        ("dest.md", "- [ ] #task Target ^target\n"),
    ]);
    let error = plan(
        &duplicate,
        &request(
            &["src:src", "src:src"],
            MergeContainer::Task(locator("dest:target")),
        ),
    )
    .expect_err("duplicate unchanged sources remain invalid");
    assert_eq!(error.code, "merge_duplicate_source");

    let nested_target = snapshot(&[
        (
            "same.md",
            "- [-] #task Retired ^closed\n  - [ ] #task Target ^target\n",
        ),
        ("open.md", "- [ ] #task Open source ^open\n"),
    ]);
    let error = plan(
        &nested_target,
        &request(
            &["same:closed", "open:open"],
            MergeContainer::Task(locator("same:target")),
        ),
    )
    .expect_err("closed members still participate in structural checks");
    assert_eq!(error.code, "merge_target_inside_source");
}

#[test]
fn unchanged_retry_skips_unrelated_preexisting_dependency_cycles() {
    let snapshot = snapshot(&[
        (
            "src.md",
            "- [-] #task Retired ^src\n\t- 🪢 **LINKS:** merged into [[dest#^target]]\n",
        ),
        ("dest.md", "- [ ] #task Target ^target\n"),
        (
            "x.md",
            "- [ ] #task X ^x\n\t- ⛓️ **DEPENDS ON:** [[y#^y]]\n",
        ),
        (
            "y.md",
            "- [ ] #task Y ^y\n\t- ⛓️ **DEPENDS ON:** [[x#^x]]\n",
        ),
    ]);
    let plan = plan(
        &snapshot,
        &request(&["src:src"], MergeContainer::Task(locator("dest:target"))),
    )
    .expect("true retry does not validate unrelated graph defects");
    assert!(plan.postimages.is_empty());
    assert_eq!(plan.records[0].outcome, MergeOutcome::Unchanged);
}

#[test]
fn relation_policy_controls_mirrors_and_preserves_existing_mentions() {
    let base = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\n"),
        ("dest.md", "- [ ] #task Target ^dst\n"),
        ("older.md", "- [ ] #task Older ^older\n"),
    ]);
    for linked_from_mentions in [false, true] {
        let plan = plan_with_policy(
            &base,
            &request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
            MergeRelationPolicy {
                linked_from_lines: true,
                linked_from_mentions,
            },
            &[],
        )
        .expect("enabled relationship mirror");
        let destination = &plan.postimages[Path::new("dest.md")];
        assert!(destination.contains("[[src#^src]] (merged into)"));
        assert!(!destination.contains("mentions"));

        let with_mention = snapshot(&[
            ("src.md", "- [ ] #task Source ^src\n"),
            (
                "dest.md",
                "- [ ] #task Target ^dst\n\t- 🪢 **LINKED FROM:** [[older#^older]] (💬 mentions)\n",
            ),
            ("older.md", "- [ ] #task Older ^older\n"),
        ]);
        let plan = plan_with_policy(
            &with_mention,
            &request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
            MergeRelationPolicy {
                linked_from_lines: true,
                linked_from_mentions,
            },
            &[],
        )
        .expect("merge keeps existing mention mirrors");
        let destination = &plan.postimages[Path::new("dest.md")];
        assert!(destination.contains("[[older#^older]] (💬 mentions)"));
        assert!(destination.contains("[[src#^src]] (merged into)"));
    }

    let disabled = plan_with_policy(
        &base,
        &request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
        MergeRelationPolicy {
            linked_from_lines: false,
            linked_from_mentions: false,
        },
        &[],
    )
    .expect("disabled mirror leaves target unchanged");
    assert!(!disabled.postimages[Path::new("dest.md")].contains("LINKED FROM"));
    assert!(disabled.postimages[Path::new("src.md")].contains("LINKS:"));
}

#[test]
fn malformed_linked_from_refuses_only_when_mirror_is_enabled() {
    let snapshot = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\n"),
        (
            "dest.md",
            "- [ ] #task Target ^dst\n\t- 🪢 **LINKED FROM:** [[broken\n",
        ),
    ]);
    let enabled = plan_with_policy(
        &snapshot,
        &request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
        MergeRelationPolicy::default(),
        &[],
    )
    .expect_err("enabled mirror cannot write through malformed input");
    assert_eq!(enabled.code, "merge_provenance_malformed");

    let disabled = plan_with_policy(
        &snapshot,
        &request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
        MergeRelationPolicy {
            linked_from_lines: false,
            linked_from_mentions: false,
        },
        &[],
    )
    .expect("disabled mirror preserves malformed input");
    assert!(disabled.postimages[Path::new("dest.md")]
        .contains("LINKED FROM:** [[broken"));
    assert!(disabled.postimages[Path::new("src.md")].contains("LINKS:"));
}

#[test]
fn closed_label_gesture_writes_only_relations_and_keeps_unrelated_entries() {
    let snapshot = snapshot(&[
        (
            "src.md",
            "- [x] #task Closed source ^src\n\t- 🪢 **LINKS:** retained [[older#^older]]\n",
        ),
        (
            "dest.md",
            "- [ ] #task Target ^dst\n\t- 🪢 **LINKED FROM:** [[older#^older]] (💬 mentions)\n",
        ),
        ("older.md", "- [ ] #task Older ^older\n"),
    ]);
    let mut request =
        request(&["src:src"], MergeContainer::Task(locator("dest:dst")));
    request.gesture = MergeGesture::Label;
    let plan =
        plan(&snapshot, &request).expect("label relation on closed task");
    assert_eq!(plan.records[0].outcome, MergeOutcome::AlreadyClosed);
    let source = &plan.postimages[Path::new("src.md")];
    assert!(source.starts_with("- [x]"));
    assert!(!source.contains("[cancelled::"));
    assert!(source.contains("retained [[older#^older]]"));
    assert!(source.contains("merged into [[dest#^dst]]"));
    let target = &plan.postimages[Path::new("dest.md")];
    assert!(target.contains("[[older#^older]] (💬 mentions)"));
    assert!(target.contains("[[src#^src]] (merged into)"));

    let json = serde_json::to_value(&plan.records[0]).expect("record JSON");
    assert_eq!(json["outcome"], "already_closed");
    assert_eq!(json["previous_status"], "x");
    assert!(json["target"].get("drawer").is_none());
    assert!(json.get("previousStatus").is_none());
}

#[test]
fn mg5_shared_dependent_dedups_to_one_target_link() {
    let snapshot = snapshot(&[
        ("first.md", "- [ ] #task First ^first\n"),
        ("second.md", "- [ ] #task Second ^second\n"),
        ("dest.md", "- [ ] #task Target ^dst\n"),
        (
            "dep.md",
            "- [ ] #task Dependent ^dep\n\t- ⛓️ **DEPENDS ON:** [[first#^first]] • [[second#^second]]\n",
        ),
    ]);
    let (staged, reports) = retire(
        &snapshot,
        &request(
            &["first:first", "second:second"],
            MergeContainer::Task(locator("dest:dst")),
        ),
    )
    .expect("retire shared dependent");
    let dependent = staged_text(&staged, "dep.md");
    let depends_on = dependent
        .lines()
        .find(|line| line.contains("DEPENDS ON"))
        .expect("repointed line")
        .to_string();
    assert_eq!(depends_on.matches("dest#^dst").count(), 1, "{dependent}");
    assert!(!dependent.contains("first#^first"), "{dependent}");
    assert!(!dependent.contains("second#^second"), "{dependent}");
    assert_eq!(
        reports[0].dependents_repointed,
        vec!["dep.md#^dep".to_string()]
    );
    assert_eq!(
        reports[1].dependents_repointed,
        vec!["dep.md#^dep".to_string()]
    );
}

#[test]
fn repoint_preserves_unresolved_links_and_field_breadcrumbs() {
    let snapshot = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\n"),
        ("dest.md", "- [ ] #task Target ^dst\n"),
        (
            "dep.md",
            concat!(
                "- [ ] #task Dependent [dependsOn:: legacy-id] ^dep\n",
                "\t- ⛓️ **DEPENDS ON:** [[src#^src]] • [[missing#^missing]]\n",
            ),
        ),
    ]);
    let plan = plan(
        &snapshot,
        &request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
    )
    .expect("resolved edge can be repointed conservatively");
    let dependent = &plan.postimages[Path::new("dep.md")];
    assert!(dependent.contains("[[dest#^dst]] • [[missing#^missing]]"));
    assert!(dependent.contains("[dependsOn::"));
    assert!(dependent.contains("legacy-id"));
    assert!(plan.warnings.iter().any(|warning| {
        warning
            .contains("unresolved or ambiguous dependency [[missing#^missing]]")
    }));
}

#[test]
fn idless_open_dependents_are_refused_for_task_and_section_merges() {
    let task_snapshot = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\n"),
        ("dest.md", "- [ ] #task Target ^dst\n"),
        (
            "dep.md",
            "- [ ] #task No id\n\t- ⛓️ **DEPENDS ON:** [[src#^src]]\n",
        ),
    ]);
    let error = plan(
        &task_snapshot,
        &request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
    )
    .expect_err("unsafe BLOCKS mirror must refuse atomically");
    assert_eq!(error.code, "merge_dependent_missing_id");
    assert!(error.message.contains("No id"));

    let section_snapshot = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\n"),
        (
            "areas/work.md",
            "---\ntype: [[area]]\n---\n## Future Work\n",
        ),
        (
            "dep.md",
            "- [ ] #task No id\n\t- ⛓️ **DEPENDS ON:** [[src#^src]]\n",
        ),
    ]);
    let section_request = request(
        &["src:src"],
        MergeContainer::Section {
            note: "areas/work".to_string(),
            selector: DrawerSelector {
                text: "Future Work".to_string(),
                exact: true,
            },
        },
    );
    let error = plan(&section_snapshot, &section_request)
        .expect_err("section cannot leave any open dependent behind");
    assert_eq!(error.code, "merge_section_has_dependents");
    assert!(error.message.contains("No id"));
}

#[test]
fn mg6_target_prerequisite_on_source_is_never_repointed() {
    let snapshot = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\n"),
        (
            "dest.md",
            "- [ ] #task Target ^dst\n\t- ⛓️ **DEPENDS ON:** [[src#^src]]\n",
        ),
    ]);
    let (staged, reports) = retire(
        &snapshot,
        &request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
    )
    .expect("target waiting on source still merges");
    // T is never repointed to itself; the edge stays for cancel recovery.
    let destination = staged_text(&staged, "dest.md");
    assert!(!destination.contains("dest#^dst"), "{destination}");
    assert!(reports[0].dependents_repointed.is_empty());
    let source = staged_text(&staged, "src.md");
    assert!(source.starts_with("- [-]"), "{source}");
}

#[test]
fn mg7_repoint_cycle_refuses_the_whole_batch() {
    let snapshot = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\n"),
        (
            "dest.md",
            "- [ ] #task Target ^dst\n\t- ⛓️ **DEPENDS ON:** [[dep#^dep]]\n",
        ),
        (
            "dep.md",
            "- [ ] #task Dependent ^dep\n\t- ⛓️ **DEPENDS ON:** [[src#^src]]\n",
        ),
    ]);
    let error = retire(
        &snapshot,
        &request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
    )
    .expect_err("cycle must refuse");
    assert_eq!(error.code, "merge_dependency_cycle");
}

#[test]
fn cycle_checks_include_project_and_inherited_resident_edges() {
    for (project_path, target, project, subproject) in [
        (
            "project.md",
            "project:target",
            concat!(
                "---\ntype: [[project]]\n---\n",
                "- [ ] #task Project ^prj\n",
                "\t- ⛓️ **DEPENDS ON:** [[dependent#^dep]]\n",
                "- [ ] #task Target ^target\n",
            ),
            None,
        ),
        (
            "project.md",
            "subproject:target",
            concat!(
                "---\ntype: [[project]]\n---\n",
                "- [ ] #task Project ^prj\n",
                "\t- ⛓️ **DEPENDS ON:** [[dependent#^dep]]\n",
            ),
            Some(concat!(
                "---\ntype: [[project]]\nparent: [[project]]\n---\n",
                "- [ ] #task Subproject ^prj\n",
                "- [ ] #task Target ^target\n",
            )),
        ),
    ] {
        let mut notes = vec![
            ("src.md", "- [ ] #task Source ^src\n"),
            (project_path, project),
            (
                "dependent.md",
                "- [ ] #task Dependent ^dep\n\t- ⛓️ **DEPENDS ON:** [[src#^src]]\n",
            ),
        ];
        if let Some(subproject) = subproject {
            notes.push(("subproject.md", subproject));
        }
        let snapshot = snapshot(&notes);
        let error = plan(
            &snapshot,
            &request(&["src:src"], MergeContainer::Task(locator(target))),
        )
        .expect_err("resident inherited edge closes a proposed cycle");
        assert_eq!(error.code, "merge_dependency_cycle");
    }
}

#[test]
fn mg8_starts_after_successors_are_reported_untouched() {
    let snapshot = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\n"),
        ("dest.md", "- [ ] #task Target ^dst\n"),
        (
            "other.md",
            "- [ ] #task Other ^other\n\t- 🪢 **LINKS:** starts after [[src#^src]]\n",
        ),
    ]);
    let labels = [crate::native::config::relations::CancelLabel {
        key: "starts after".to_string(),
        name: "starts after".to_string(),
    }];
    let (staged, reports) = retire_with_successor_labels(
        &snapshot,
        &request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
        &labels,
    )
    .expect("retire with successor");
    assert_eq!(
        reports[0].successors_orphaned,
        vec!["other.md#^other".to_string()]
    );
    let other = staged_text(&staged, "other.md");
    assert!(other.contains("starts after [[src#^src]]"), "{other}");
}

#[test]
fn successor_orphans_use_only_the_supplied_normalized_labels() {
    let snapshot = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\n"),
        ("dest.md", "- [ ] #task Target ^dst\n"),
        (
            "other.md",
            "- [ ] #task Other ^other\n\t- 🪢 **LINKS:** follows [[src#^src]]\n",
        ),
    ]);
    let labels = [crate::native::config::relations::CancelLabel {
        key: "follows".to_string(),
        name: "follows".to_string(),
    }];
    let (_, reports) = retire_with_successor_labels(
        &snapshot,
        &request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
        &labels,
    )
    .expect("configured custom successor relation");
    assert_eq!(reports[0].successors_orphaned, ["other.md#^other"]);

    let (_, reports) = retire(
        &snapshot,
        &request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
    )
    .expect("no configured labels");
    assert!(reports[0].successors_orphaned.is_empty());
}

#[test]
fn mg9_repoint_lands_before_cancel_without_transient_recovery() {
    let snapshot = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\n"),
        ("dest.md", "- [ ] #task Target ^dst\n"),
        (
            "dep.md",
            "- [ ] #task Dependent ^dep\n\t- ⛓️ **DEPENDS ON:** [[src#^src]]\n",
        ),
    ]);
    let (staged, _) = retire(
        &snapshot,
        &request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
    )
    .expect("retire");
    // The repointed dependent already waits on T and never received a
    // transient recovery stamp or offer.
    let dependent = staged_text(&staged, "dep.md");
    assert!(dependent.contains("[[dest#^dst]]"), "{dependent}");
    assert!(!dependent.contains("[unblocked::]"), "{dependent}");
    assert!(dependent.starts_with("- [ ]"), "{dependent}");
}

#[test]
fn mg9_source_prerequisites_are_retained_and_reported() {
    let snapshot = snapshot(&[
        (
            "src.md",
            "- [ ] #task Source ^src\n\t- ⛓️ **DEPENDS ON:** [[pre#^pre]]\n",
        ),
        ("pre.md", "- [ ] #task Prerequisite ^pre\n"),
        ("dest.md", "- [ ] #task Target ^dst\n"),
    ]);
    let (staged, reports) = retire(
        &snapshot,
        &request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
    )
    .expect("retire with prerequisites");
    assert_eq!(
        reports[0].prerequisites_left,
        vec!["pre.md#^pre".to_string()]
    );
    // Prerequisites are retained on S, never transferred to T.
    let source = staged_text(&staged, "src.md");
    assert!(source.contains("pre#^pre"), "{source}");
    let destination = staged_text(&staged, "dest.md");
    assert!(!destination.contains("DEPENDS ON"), "{destination}");
}

#[test]
fn mg13_section_merge_with_open_dependents_refuses() {
    let snapshot = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\n"),
        (
            "areas/work.md",
            "---\ntype: [[area]]\n---\n## Future Work\n- Existing\n",
        ),
        (
            "dep.md",
            "- [ ] #task Dependent ^dep\n\t- ⛓️ **DEPENDS ON:** [[src#^src]]\n",
        ),
    ]);
    let error = retire(
        &snapshot,
        &request(
            &["src:src"],
            MergeContainer::Section {
                note: "areas/work".to_string(),
                selector: DrawerSelector {
                    text: "future-work".to_string(),
                    exact: false,
                },
            },
        ),
    )
    .expect_err("section with dependents must refuse");
    assert_eq!(error.code, "merge_section_has_dependents");
}

#[test]
fn mg13_section_provenance_writes_one_cancel_log_entry() {
    let snapshot = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\n"),
        (
            "areas/work.md",
            "---\ntype: [[area]]\n---\n## Future Work\n- Existing\n",
        ),
    ]);
    let (staged, _) = retire(
        &snapshot,
        &request(
            &["src:src"],
            MergeContainer::Section {
                note: "areas/work".to_string(),
                selector: DrawerSelector {
                    text: "future-work".to_string(),
                    exact: false,
                },
            },
        ),
    )
    .expect("section merge without dependents");
    let source = staged_text(&staged, "src.md");
    assert!(source.starts_with("- [-]"), "{source}");
    assert!(
        source.contains("📥 merged into [[areas/work#Future Work]]"),
        "{source}"
    );
    // Exactly one provenance entry: no task relation, no duplicate log.
    assert!(!source.contains("🪢 **LINKS:**"), "{source}");
    assert_eq!(
        source
            .matches("📥 merged into [[areas/work#Future Work]]")
            .count(),
        1,
        "{source}"
    );
}

#[test]
fn section_batch_logs_each_source_and_retry_is_write_free() {
    let snapshot = snapshot(&[
        (
            "src.md",
            concat!(
                "- [ ] #task First ^first\n",
                "- [ ] #task Second ^second\n",
            ),
        ),
        (
            "areas/work.md",
            "---\ntype: [[area]]\n---\n## Future Work\n",
        ),
    ]);
    let request = request(
        &["src:first", "src:second"],
        MergeContainer::Section {
            note: "areas/work".to_string(),
            selector: DrawerSelector {
                text: "Future Work".to_string(),
                exact: true,
            },
        },
    );
    let first = plan(&snapshot, &request).expect("batch section merge");
    let source = &first.postimages[Path::new("src.md")];
    assert_eq!(
        source
            .matches("📥 merged into [[areas/work#Future Work]]")
            .count(),
        2,
        "each source owns one dated entry: {source}"
    );
    let mut after = snapshot.notes.clone();
    after.extend(first.postimages);
    let second = plan(&VaultSnapshot::from_notes(after), &request)
        .expect("same section batch retry");
    assert!(second.postimages.is_empty());
    assert!(second
        .records
        .iter()
        .all(|record| record.outcome == MergeOutcome::Unchanged));
}

#[test]
fn section_retry_ignores_matching_prose_fences_and_later_task_logs() {
    let snapshot = snapshot(&[
        (
            "src.md",
            concat!(
                "- [-] #task Closed source ^src\n",
                "  - prose 📥 merged into [[areas/work#Future Work]]\n",
                "  - ❌ **CANCEL LOG**\n",
                "  ```text\n",
                "    - *2026-11-03* — 📥 merged into [[areas/work#Future Work]]\n",
                "  ```\n",
                "- [-] #task Later ^later\n",
                "  - ❌ **CANCEL LOG**\n",
                "    - *2026-11-03* — 📥 merged into [[areas/work#Future Work]]\n",
            ),
        ),
        (
            "areas/work.md",
            "---\ntype: [[area]]\n---\n## Future Work\n",
        ),
    ]);
    let request = request(
        &["src:src"],
        MergeContainer::Section {
            note: "areas/work".to_string(),
            selector: DrawerSelector {
                text: "Future Work".to_string(),
                exact: true,
            },
        },
    );
    let error = plan(&snapshot, &request)
        .expect_err("only source's direct dated log can establish identity");
    assert_eq!(error.code, "merge_source_closed");
}

#[test]
fn mg16_rerun_against_same_target_is_unchanged() {
    let snapshot = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\n"),
        ("dest.md", "- [ ] #task Target ^dst\n"),
    ]);
    let (staged, _) = retire(
        &snapshot,
        &request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
    )
    .expect("first merge");
    let retired = VaultSnapshot::from_notes(staged);
    let (outcome, disposition) = classify(
        &retired,
        "src:src",
        MergeContainer::Task(locator("dest:dst")),
        "merged into",
        MergeGesture::Explicit,
    );
    assert_eq!(outcome, SourceOutcome::Unchanged);
    assert_eq!(disposition, SourceDisposition::Closed);
}

#[test]
fn closed_sources_classify_for_label_gestures_and_refuse_explicit() {
    let before = snapshot(&[
        ("closed.md", "- [x] #task Closed ^closed\n"),
        ("dest.md", "- [ ] #task Target ^dst\n"),
    ]);
    let (outcome, _) = classify(
        &before,
        "closed:closed",
        MergeContainer::Task(locator("dest:dst")),
        "merged into",
        MergeGesture::Label,
    );
    assert_eq!(outcome, SourceOutcome::AlreadyClosed);

    let done_with_edge = snapshot(&[
        (
            "closed.md",
            "- [x] #task Done source ^closed\n\t- 🪢 **LINKS:** merged into [[dest#^dst]]\n",
        ),
        ("dest.md", "- [ ] #task Target ^dst\n"),
    ]);
    let (outcome, disposition) = classify(
        &done_with_edge,
        "closed:closed",
        MergeContainer::Task(locator("dest:dst")),
        "merged into",
        MergeGesture::Label,
    );
    assert_eq!(outcome, SourceOutcome::AlreadyClosed);
    assert_eq!(disposition, SourceDisposition::Closed);

    let error = retire(
        &before,
        &request(
            &["closed:closed"],
            MergeContainer::Task(locator("dest:dst")),
        ),
    )
    .expect_err("explicit closed source must refuse");
    assert_eq!(error.code, "merge_source_closed");
}

#[test]
fn explicit_merge_into_new_target_names_existing_merge_tombstone() {
    let before = snapshot(&[
        (
            "src.md",
            "- [-] #task Done work ^src\n\t- 🪢 **LINKS:** merged into [[old#^dst]]\n",
        ),
        ("old.md", "- [ ] #task Old target ^dst\n"),
        ("new.md", "- [ ] #task New target ^dst\n"),
    ]);
    let error = plan(
        &before,
        &request(&["src:src"], MergeContainer::Task(locator("new:dst"))),
    )
    .expect_err("already-merged source must refuse the new target");
    assert_eq!(error.code, "merge_source_closed");
    assert!(
        error.message.contains("Done work"),
        "names the cleaned source title: {}",
        error.message
    );
    assert!(
        error.message.contains("Old target"),
        "names the existing target title: {}",
        error.message
    );
    assert!(
        error.message.contains("old.md"),
        "names the existing target note: {}",
        error.message
    );
}

#[test]
fn mg19_shared_key_with_merge_labels_drops_everywhere_once() {
    let (dropped, warnings) = overlapping_effect_keys(
        "relations.yaml",
        &[
            (
                "cancel_labels",
                vec![("merged-into".to_string(), "merged into".to_string())],
            ),
            ("complete_labels", vec![]),
            ("successor_labels", vec![]),
            (
                "merge_labels",
                vec![("merged-into".to_string(), "merged into".to_string())],
            ),
        ],
    );
    assert_eq!(dropped, BTreeSet::from(["merged-into".to_string()]));
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("merge_labels"), "{}", warnings[0]);
}

#[test]
fn mg19_three_list_warnings_stay_byte_identical() {
    let (_, warnings) = overlapping_effect_keys(
        "cfg.yaml",
        &[
            (
                "cancel_labels",
                vec![("done".to_string(), "done".to_string())],
            ),
            (
                "complete_labels",
                vec![("done".to_string(), "done".to_string())],
            ),
            ("successor_labels", vec![]),
        ],
    );
    assert_eq!(
        warnings,
        vec![
            "relations: “done” is in both cancel_labels and complete_labels in cfg.yaml; it closes nothing"
                .to_string()
        ]
    );
    let (_, warnings) = overlapping_effect_keys(
        "cfg.yaml",
        &[
            (
                "cancel_labels",
                vec![("later".to_string(), "later".to_string())],
            ),
            ("complete_labels", vec![]),
            (
                "successor_labels",
                vec![("later".to_string(), "later".to_string())],
            ),
        ],
    );
    assert_eq!(
        warnings,
        vec![
            "relations: “later” is in more than one of cancel_labels, complete_labels, successor_labels in cfg.yaml; it has no effect"
                .to_string()
        ]
    );
}

#[test]
fn mg10_existing_target_link_suppresses_merge_offer() {
    let snapshot = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\n"),
        ("dest.md", "- [ ] #task Target ^dst\n"),
        (
            "daily/2026-11-03.md",
            "## Pomodoros\n- [ ] () — BUILD\n\t- [[src#^src]]\n\t- [[dest#^dst]]\n",
        ),
    ]);
    let plan = plan(
        &snapshot,
        &request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
    )
    .expect("merge with existing target link");
    assert!(plan.records[0].offers.is_empty());
    assert_eq!(plan.records[0].links_pruned, 1);
    assert!(plan.postimages[Path::new("daily/2026-11-03.md")]
        .contains("[[dest#^dst]]"));
}

#[test]
fn mg11_blocked_target_is_not_offered_after_pruning() {
    let snapshot = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\n"),
        ("dest.md", "- [?] #task Target ^dst\n"),
        (
            "daily/2026-11-03.md",
            "## Pomodoros\n- [ ] () — BUILD\n\t- [[src#^src]]\n",
        ),
    ]);
    let plan = plan(
        &snapshot,
        &request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
    )
    .expect("blocked target does not refuse the merge");
    assert!(plan.records[0].offers.is_empty());
    assert!(
        plan.postimages[Path::new("daily/2026-11-03.md")].contains("— BUILD\n")
    );
    assert!(!plan.postimages[Path::new("daily/2026-11-03.md")]
        .contains("[[src#^src]]"));
}

#[test]
fn mg12_offer_uses_earliest_removed_slot_and_selected_pick_semantics() {
    let snapshot = snapshot(&[
        ("first.md", "- [ ] #task First source ^first\n"),
        ("second.md", "- [ ] #task Second source ^second\n"),
        ("dest.md", "- [ ] #task Target ^dst\n"),
        (
            "daily/2026-11-03.md",
            "## Pomodoros\n- [ ] () — EARLY\n\t- [[second#^second]]\n- [ ] () — LATE\n\t- [[first#^first]]\n",
        ),
    ]);
    let mut request = request(
        &["first:first", "second:second"],
        MergeContainer::Task(locator("dest:dst")),
    );
    request.selection = SuccessorSelection::All;
    let plan = plan(&snapshot, &request).expect("merge with picked offer");
    assert_eq!(plan.records[0].offers.len(), 0);
    assert_eq!(plan.records[1].offers.len(), 1);
    let offer = &plan.records[1].offers[0];
    assert_eq!(offer.group, "hand_off");
    assert_eq!(offer.entry_name, "EARLY");
    assert!(offer.selected);
    assert_eq!(offer.merged_from.as_deref(), Some("second.md#^second"));
    assert_eq!(
        plan.postimages[Path::new("daily/2026-11-03.md")],
        "## Pomodoros\n- [ ] () — EARLY\n\t- [[dest#^dst]]\n- [ ] () — LATE\n"
    );
    assert!(plan.postimages[Path::new("dest.md")]
        .starts_with("- [*] #task Target [fresh:: 2026-11-03] ^dst"));
}

#[test]
fn mg12_picked_slot_rebases_after_same_daily_note_edits_and_source_permutations(
) {
    let snapshot = snapshot(&[
        (
            "daily/2026-11-03.md",
            concat!(
                "- [ ] #task Source A ^a\n",
                "- [ ] #task Target ^target\n",
                "- [ ] #task Source B ^b\n",
                "## Pomodoros\n",
                "- [ ] () — EARLY\n",
                "\t- [[before#^before]]\n",
                "\t- [[#^a]]\n",
                "\t- [[after#^after]]\n",
                "- [ ] () — LATER\n",
                "\t- [[#^b]]\n",
            ),
        ),
        ("before.md", "- [ ] #task Before ^before\n"),
        ("after.md", "- [ ] #task After ^after\n"),
    ]);
    let expected_ledger = concat!(
        "## Pomodoros\n",
        "- [ ] () — EARLY\n",
        "\t- [[before#^before]]\n",
        "\t- [[2026-11-03#^target]]\n",
        "\t- [[after#^after]]\n",
        "- [ ] () — LATER\n",
    );
    for order in [
        ["daily/2026-11-03:a", "daily/2026-11-03:b"],
        ["daily/2026-11-03:b", "daily/2026-11-03:a"],
    ] {
        let mut request = request(
            &order,
            MergeContainer::Task(locator("daily/2026-11-03:target")),
        );
        request.selection = SuccessorSelection::All;
        let plan = plan(&snapshot, &request)
            .expect("same-daily-note picked merge offer");
        let daily = &plan.postimages[Path::new("daily/2026-11-03.md")];
        let ledger = &daily[daily.find("## Pomodoros").expect("ledger")..];
        assert_eq!(ledger, expected_ledger);
        let a_record = plan
            .records
            .iter()
            .find(|record| record.source.block_id == "a")
            .expect("source A record");
        assert_eq!(a_record.offers.len(), 1);
        assert!(a_record.offers[0].selected);
        assert!(plan
            .write_order
            .iter()
            .all(|path| { plan.postimages.contains_key(path) }));
    }
}

#[test]
fn mg12_picked_slot_preserves_plain_notes_between_removed_links() {
    let snapshot = snapshot(&[
        ("first.md", "- [ ] #task First ^first\n"),
        ("second.md", "- [ ] #task Second ^second\n"),
        ("dest.md", "- [ ] #task Target ^dst\n"),
        (
            "daily/2026-11-03.md",
            concat!(
                "## Pomodoros\n- [ ] () — BUILD\n",
                "  - Before\n  - [[first#^first]]\n",
                "  - Between\n    - Child of between\n",
                "  - [[second#^second]]\n  - After\n",
            ),
        ),
    ]);
    let mut request = request(
        &["second:second", "first:first"],
        MergeContainer::Task(locator("dest:dst")),
    );
    request.selection = SuccessorSelection::All;
    let result = plan(&snapshot, &request).expect("selected merge");
    assert_eq!(
        result.postimages[Path::new("daily/2026-11-03.md")],
        concat!(
            "## Pomodoros\n- [ ] () — BUILD\n",
            "  - Before\n  - [[dest#^dst]]\n",
            "  - Between\n    - Child of between\n  - After\n",
        )
    );
}

#[test]
fn mg16_public_planner_rerun_has_no_writes_and_serializes_source_order() {
    let before = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\n"),
        ("dest.md", "- [ ] #task Target ^dst\n"),
    ]);
    let request =
        request(&["src:src"], MergeContainer::Task(locator("dest:dst")));
    let first = plan(&before, &request).expect("first merge");
    let mut after = before.notes.clone();
    after.extend(first.postimages.clone());
    let after = VaultSnapshot::from_notes(after);
    let second = plan(&after, &request).expect("idempotent rerun");
    assert!(second.postimages.is_empty());
    assert_eq!(second.records[0].outcome, MergeOutcome::Unchanged);
    assert_eq!(
        serde_json::to_value(&first.records)
            .expect("serializable merge records")[0]["source"]["line"],
        1
    );
}

#[test]
fn merge_record_and_offer_json_use_snake_case_and_omit_absent_fields() {
    let before = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\n"),
        ("dest.md", "- [ ] #task Target ^dst\n"),
        (
            "daily/2026-11-03.md",
            "## Pomodoros\n- [ ] () — BUILD\n\t- [[src#^src]]\n",
        ),
    ]);
    let mut merge_request =
        request(&["src:src"], MergeContainer::Task(locator("dest:dst")));
    merge_request.selection = SuccessorSelection::All;
    let merge_plan =
        plan(&before, &merge_request).expect("record with an offer");
    let record =
        serde_json::to_value(&merge_plan.records[0]).expect("record JSON");
    for key in [
        "previous_status",
        "moved_lines",
        "moved_notes",
        "dependents_repointed",
        "prerequisites_left",
        "successors_orphaned",
        "links_pruned",
    ] {
        assert!(record.get(key).is_some(), "missing {key}: {record}");
    }
    assert!(record.get("previousStatus").is_none());
    assert_eq!(record["source"]["block_id"], "src");
    assert_eq!(record["target"]["block_id"], "dst");
    assert!(record["target"].get("drawer").is_none());
    assert!(record["target"].get("section").is_none());
    let offer = &record["offers"][0];
    assert_eq!(offer["entry_name"], "BUILD");
    assert_eq!(offer["merged_from"], "src.md#^src");
    assert!(offer.get("entryName").is_none());
    assert!(offer.get("mergedFrom").is_none());

    let section_snapshot = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\n"),
        (
            "areas/work.md",
            "---\ntype: [[area]]\n---\n## Future Work\n",
        ),
    ]);
    let section_plan = plan(
        &section_snapshot,
        &request(
            &["src:src"],
            MergeContainer::Section {
                note: "areas/work".to_string(),
                selector: DrawerSelector {
                    text: "Future Work".to_string(),
                    exact: true,
                },
            },
        ),
    )
    .expect("section target record");
    let section_record = serde_json::to_value(&section_plan.records[0])
        .expect("section record JSON");
    assert!(section_record["target"].get("block_id").is_none());
    assert_eq!(section_record["target"]["section"], "Future Work");

    let no_origin = MergeOffer {
        key: "target".to_string(),
        n: 1,
        group: "hand_off".to_string(),
        selected: false,
        entry_name: "BUILD".to_string(),
        entry_role: "slot".to_string(),
        merged_from: None,
    };
    assert!(serde_json::to_value(no_origin)
        .expect("optional offer JSON")
        .get("merged_from")
        .is_none());
}

#[test]
fn mg17_production_reconciliation_is_a_noop_after_merge() {
    let before = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\n"),
        ("dest.md", "- [ ] #task Target ^dst\n"),
        (
            "dep.md",
            "- [ ] #task Dependent ^dep\n\t- ⛓️ **DEPENDS ON:** [[src#^src]]\n",
        ),
    ]);
    let request =
        request(&["src:src"], MergeContainer::Task(locator("dest:dst")));
    let first = plan(&before, &request).expect("merge plan");
    let second = plan(&before, &request).expect("repeatable merge plan");
    assert_eq!(first, second);
    let mut merged = before.notes.clone();
    merged.extend(first.postimages);
    let settings = note_tasks::read_settings(Path::new("/no-bob-vault"));
    let reconciled =
        crate::native::task_status_hooks::reconcile_merge_snapshot_for_tests(
            &merged, &settings,
        );
    assert_eq!(reconciled, merged);
}

#[test]
fn mg17_closed_nested_task_copy_strips_identity_and_reconcile_is_noop() {
    let before = snapshot(&[
        (
            "src.md",
            concat!(
                "- [ ] #task Source ^src\n",
                "\t- [x] #task Nested [id:: src__nested] ^nested\n",
                "\t\t- ⛓️ **DEPENDS ON:** [[pre#^pre]]\n",
            ),
        ),
        ("pre.md", "- [ ] #task Prerequisite [id:: pre__pre] ^pre\n"),
        ("dest.md", "- [ ] #task Target ^dst\n"),
    ]);
    let request =
        request(&["src:src"], MergeContainer::Task(locator("dest:dst")));
    let first = plan(&before, &request).expect("merge plan");
    let destination = first
        .postimages
        .get(Path::new("dest.md"))
        .expect("destination postimage");
    assert!(
        destination.contains("\t\t- [x] #task Nested"),
        "{destination}"
    );
    assert!(
        !destination.contains("[id:: src__nested]"),
        "copy strips [id::]: {destination}"
    );
    assert!(
        !destination.contains("^nested"),
        "copy strips nested ^id: {destination}"
    );
    assert!(
        destination.contains("DEPENDS ON:** [[pre#^pre]]"),
        "{destination}"
    );
    let source = first
        .postimages
        .get(Path::new("src.md"))
        .map(String::as_str)
        .unwrap_or(before.notes[Path::new("src.md")].as_str());
    assert!(source.contains("[id:: src__nested]"), "{source}");
    assert!(source.contains("^nested"), "{source}");
    let mut merged = before.notes.clone();
    merged.extend(first.postimages);
    let settings = note_tasks::read_settings(Path::new("/no-bob-vault"));
    let reconciled =
        crate::native::task_status_hooks::reconcile_merge_snapshot_for_tests(
            &merged, &settings,
        );
    assert_eq!(reconciled, merged);
}

#[test]
fn mg18_archive_planner_repairs_merged_from_to_tombstone_archive() {
    let vault = tempfile::tempdir().expect("temporary vault");
    let source = "---\ntype: [[area]]\n---\n\n- [-] #task Source [created:: 2026-10-20] [cancelled:: 2026-11-03] ^src\n\t- 🪢 **LINKS:** merged into [[dest#^dst]]\n";
    let target_note = "---\ntype: [[area]]\n---\n\n- [ ] #task Target ^dst\n\t- 🪢 **LINKED FROM:** [[src#^src]] (merged into)\n";
    std::fs::write(vault.path().join("src.md"), source)
        .expect("write source fixture");
    std::fs::write(vault.path().join("dest.md"), target_note)
        .expect("write target fixture");
    let postimages =
        crate::native::collect_done::collection_postimages_for_tests(
            vault.path(),
            1,
        )
        .expect("production archive planner");
    let target = postimages
        .get(Path::new("dest.md"))
        .expect("rewritten target postimage");
    assert!(target.contains("done/src_done#^src"), "{target}");
    assert!(!target.contains("[[src#^src]]"), "{target}");
    let mut archived = BTreeMap::from([
        (PathBuf::from("src.md"), source.to_string()),
        (PathBuf::from("dest.md"), target_note.to_string()),
    ]);
    archived.extend(postimages);
    let index = NoteIndex::from_paths(archived.keys().cloned());
    let targets = block_link_targets(&archived);
    assert!(targets
        .contains(&(PathBuf::from("done/src_done.md"), "src".to_string(),)));
    for (path, block_id) in targets {
        assert!(
            block_ids(&archived).contains(&(path.clone(), block_id.clone())),
            "archive link identity must resolve: {}#^{}",
            path.display(),
            block_id,
        );
        assert!(matches!(
            index.resolve_detailed(None, &path.with_extension("").to_string_lossy()),
            LinkResolution::Found(resolved) if resolved == path
        ));
    }
}

#[test]
fn mg_successful_merge_fixtures_hold_identity_reconcile_and_repeatability() {
    let mut fixtures = vec![
        (
            "MG1 title only",
            snapshot(&[
                ("src.md", "- [ ] #task Source ^src\n"),
                ("dest.md", "- [ ] #task Target ^dst\n"),
            ]),
            request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
        ),
        (
            "MG2 nested drawer",
            snapshot(&[
                ("src.md", "- [ ] #task Source ^src\n  - FUTURE WORK\n    - Detail ^detail\n"),
                ("dest.md", "- [ ] #task Target ^dst\n"),
            ]),
            request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
        ),
        (
            "MG3 source-owned anchors",
            snapshot(&[
                ("src.md", "- [ ] #task Source ^src\n  - Detail ^detail\n"),
                ("dest.md", "- [ ] #task Target ^dst\n  - Existing ^detail\n"),
                ("daily.md", "See [[src#^src]] and [[src#^detail]].\n"),
            ]),
            request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
        ),
        (
            "MG4 tables and fences",
            snapshot(&[
                ("src.md", "- [ ] #task Source ^src\n  - Table\n    | A | B |\n    | - | - |\n    ```text\n    body\n    ```\n"),
                ("dest.md", "- [ ] #task Target ^dst\n"),
            ]),
            request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
        ),
        (
            "MG4 wrapper-free authored body",
            snapshot(&[
                (
                    "src.md",
                    "- [ ] #task Source ^src\n  Paragraph continuation before children.\n\n  | A | B |\n  | - | - |\n  ```text\n  body\n  ```\n\t- 🪢 **LINKS:** merged into [[dest#^dst]]\n  - Authored child after the managed log\n",
                ),
                ("dest.md", "- [ ] #task Target ^dst\n"),
            ]),
            request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
        ),
        (
            "MG5 and MG9 dependent DAG",
            snapshot(&[
                ("src.md", "- [ ] #task Source [id:: src__src] ^src\n\t- 🧱 **BLOCKS:** [[dep#^dep]]\n"),
                ("dest.md", "- [ ] #task Target [id:: dest__dst] ^dst\n"),
                ("dep.md", "- [ ] #task Dependent [id:: dep__dep] [dependsOn:: src__src] ^dep\n\t- ⛓️ **DEPENDS ON:** [[src#^src]]\n\t- 🧱 **BLOCKS:** [[leaf#^leaf]]\n"),
                ("leaf.md", "- [ ] #task Leaf [id:: leaf__leaf] [dependsOn:: dep__dep] ^leaf\n\t- ⛓️ **DEPENDS ON:** [[dep#^dep]]\n"),
            ]),
            request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
        ),
        (
            "MG6 target prerequisite",
            snapshot(&[
                ("src.md", "- [ ] #task Source [id:: src__src] ^src\n\t- 🧱 **BLOCKS:** [[dest#^dst]]\n"),
                ("dest.md", "- [ ] #task Target [id:: dest__dst] [dependsOn:: src__src] ^dst\n\t- ⛓️ **DEPENDS ON:** [[src#^src]]\n"),
            ]),
            request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
        ),
        (
            "MG8 successor relation",
            snapshot(&[
                ("src.md", "- [ ] #task Source ^src\n\t- 🪢 **LINKED FROM:** [[other#^other]] (starts after)\n"),
                ("dest.md", "- [ ] #task Target ^dst\n"),
                ("other.md", "- [ ] #task Other ^other\n\t- 🪢 **LINKS:** starts after [[src#^src]]\n"),
            ]),
            request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
        ),
        (
            "MG9 source prerequisite",
            snapshot(&[
                ("src.md", "- [ ] #task Source [id:: src__src] [dependsOn:: pre__pre] ^src\n\t- ⛓️ **DEPENDS ON:** [[pre#^pre]]\n"),
                ("pre.md", "- [ ] #task Prerequisite [id:: pre__pre] ^pre\n\t- 🧱 **BLOCKS:** [[src#^src]]\n"),
                ("dest.md", "- [ ] #task Target ^dst\n"),
            ]),
            request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
        ),
        (
            "MG10 target already linked",
            snapshot(&[
                ("src.md", "- [ ] #task Source ^src\n"),
                ("dest.md", "- [ ] #task Target ^dst\n"),
                ("daily/2026-11-03.md", "## Pomodoros\n- [ ] () — BUILD\n\t- [[src#^src]]\n\t- [[dest#^dst]]\n"),
            ]),
            request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
        ),
        (
            "MG11 blocked target",
            snapshot(&[
                ("src.md", "- [ ] #task Source ^src\n"),
                ("dest.md", "- [?] #task Target ^dst\n"),
                ("daily/2026-11-03.md", "## Pomodoros\n- [ ] () — BUILD\n\t- [[src#^src]]\n"),
            ]),
            request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
        ),
        (
            "MG12 selected ledger offer",
            snapshot(&[
                ("src.md", "- [ ] #task Source ^src\n"),
                ("dest.md", "- [ ] #task Target ^dst\n"),
                ("daily/2026-11-03.md", "## Pomodoros\n- [ ] () — BUILD\n\t- [[src#^src]]\n"),
            ]),
            {
                let mut request = request(
                    &["src:src"],
                    MergeContainer::Task(locator("dest:dst")),
                );
                request.selection = SuccessorSelection::All;
                request
            },
        ),
        (
            "MG13 section destination",
            snapshot(&[
                ("src.md", "- [ ] #task Source ^src\n"),
                ("areas/work.md", "---\ntype: [[area]]\n---\n## Future Work\n- Existing\n"),
            ]),
            request(
                &["src:src"],
                MergeContainer::Section {
                    note: "areas/work".to_string(),
                    selector: DrawerSelector {
                        text: "future-work".to_string(),
                        exact: false,
                    },
                },
            ),
        ),
        (
            "MG15 exact section creation",
            snapshot(&[
                ("src.md", "- [ ] #task Source ^src\n"),
                ("areas/work.md", "---\ntype: [[area]]\n---\n"),
            ]),
            request(
                &["src:src"],
                MergeContainer::Section {
                    note: "areas/work".to_string(),
                    selector: DrawerSelector {
                        text: "Exact Title".to_string(),
                        exact: true,
                    },
                },
            ),
        ),
        (
            "MG16 mixed unchanged and new sources",
            snapshot(&[
                ("old.md", "- [-] #task Old [cancelled:: 2026-11-02] ^old\n\t- 🪢 **LINKS:** merged into [[dest#^dst]]\n"),
                ("new.md", "- [ ] #task New ^new\n"),
                ("dest.md", "- [ ] #task Target ^dst\n\t- 🪢 **LINKED FROM:** [[old#^old]] (merged into)\n"),
            ]),
            request(&["old:old", "new:new"], MergeContainer::Task(locator("dest:dst"))),
        ),
        (
            "MG17 production reconciliation",
            snapshot(&[
                ("src.md", "- [ ] #task Source ^src\n"),
                ("dest.md", "- [ ] #task Target ^dst\n"),
                ("dep.md", "- [ ] #task Dependent ^dep\n\t- ⛓️ **DEPENDS ON:** [[src#^src]]\n"),
            ]),
            request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
        ),
        (
            "MG20 typed source groups",
            snapshot(&[
                ("one.md", "- [ ] #task One ^one\n"),
                ("two.md", "- [ ] #task Two ^two\n"),
                ("dest.md", "- [ ] #task Target ^dst\n\t- Existing\n"),
            ]),
            request(&["one:one", "two:two"], MergeContainer::Task(locator("dest:dst"))),
        ),
        (
            "drawer destination",
            snapshot(&[
                ("src.md", "- [ ] #task Source ^src\n"),
                ("dest.md", "- [ ] #task Target ^dst\n\t- REQUIREMENTS\n\t\t- Existing\n"),
            ]),
            request(
                &["src:src"],
                MergeContainer::Drawer(
                    locator("dest:dst"),
                    DrawerSelector {
                        text: "requirements".to_string(),
                        exact: false,
                    },
                ),
            ),
        ),
    ];

    // Exercise source ordering, same-note copies, cross-note copies, and both
    // common indentation units with generated fixture variants.
    for indent in ["  ", "\t"] {
        for same_note in [false, true] {
            let destination = if same_note { "src.md" } else { "dest.md" };
            let mut notes = vec![(
                "src.md",
                format!("- [ ] #task Source ^src\n{indent}- Detail ^detail\n"),
            )];
            if !same_note {
                notes
                    .push(("dest.md", "- [ ] #task Target ^dst\n".to_string()));
            } else {
                notes[0]
                    .1
                    .push_str("- [ ] #task Target ^dst\n  - Existing\n");
            }
            notes.push((
                "daily.md",
                "See [[src#^src]] and [[src#^detail]].\n".to_string(),
            ));
            let before = VaultSnapshot::from_notes(
                notes
                    .into_iter()
                    .map(|(path, text)| (PathBuf::from(path), text)),
            );
            fixtures.push((
                "generated indentation and note roles",
                before,
                request(
                    &["src:src"],
                    MergeContainer::Task(locator(&format!(
                        "{}:dst",
                        destination.trim_end_matches(".md")
                    ))),
                ),
            ));
        }
    }
    for order in [["one:one", "two:two"], ["two:two", "one:one"]] {
        fixtures.push((
            "generated source order",
            snapshot(&[
                ("one.md", "- [ ] #task One ^one\n"),
                ("two.md", "- [ ] #task Two ^two\n"),
                ("dest.md", "- [ ] #task Target ^dst\n"),
            ]),
            request(&order, MergeContainer::Task(locator("dest:dst"))),
        ));
    }

    for (case, before, request) in fixtures {
        assert_planner_invariants(case, &before, &request);
    }
}

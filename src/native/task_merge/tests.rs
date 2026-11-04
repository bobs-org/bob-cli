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
    };
    prepare_task_merge(snapshot, request, &context)
}

fn plan(
    snapshot: &VaultSnapshot,
    request: &MergeRequest,
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
        relation_policy: MergeRelationPolicy::default(),
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
            } else {
                format!("{} notes moved", record.moved_notes)
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
            let renames = first
                .renamed_block_ids
                .iter()
                .filter(|rename| {
                    rename.path == target_path
                        && rename.old_block_id == block_id
                })
                .collect::<Vec<_>>();
            if renames.is_empty() {
                assert!(
                    after_blocks
                        .contains(&(target_path.clone(), block_id.clone())),
                    "{case}: moved anchor identity disappeared: {}#^{}",
                    target_path.display(),
                    block_id,
                );
            } else {
                for rename in renames {
                    assert!(
                        after_blocks.contains(&(
                            target_path.clone(),
                            rename.new_block_id.clone(),
                        )),
                        "{case}: renamed anchor identity disappeared: {}#^{}",
                        target_path.display(),
                        rename.new_block_id,
                    );
                }
            }
            continue;
        }
        assert!(
            after_blocks.contains(&(path.clone(), block_id.clone())),
            "{case}: previously linked identity disappeared: {}#^{}",
            path.display(),
            block_id,
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
fn mg3_descendant_anchor_collision_repairs_external_links() {
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
    assert!(
        post(&plan, "dest.md").contains("\t\t- Note ^detail-1"),
        "{}",
        post(&plan, "dest.md")
    );
    assert_eq!(post(&plan, "daily.md"), "See [[dest#^detail-1]]\n");
    assert_eq!(plan.renamed_block_ids.len(), 1);
}

#[test]
fn colliding_descendant_ids_are_scoped_to_each_source_note() {
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
    assert!(destination.contains("\t\t- Note ^detail-1"));
    assert!(destination.contains("\t\t- Note ^detail-2"));
    assert_eq!(
        post(&plan, "daily.md"),
        "[[dest#^detail-1]] and [[dest#^detail-2]]\n"
    );
    assert_eq!(plan.renamed_block_ids.len(), 2);
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
fn same_note_destinations_and_nested_sources_are_rebased_after_cuts() {
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
        "- [ ] #task Parent ^parent\n  - [ ] #task Source ^source\n- [ ] #task Target ^target\n  - Source\n    - Note\n"
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
    let source_postimage = post(&plan, "src.md");
    assert!(!source_postimage.contains("Authored note"));
    assert!(source_postimage.contains("🪢 **LINKS:** [[other#^related]]"));
    assert!(source_postimage.contains("🛠️ **WORK LOG**"));
    assert!(source_postimage.contains("❌ **CANCEL LOG**"));
    assert!(source_postimage.contains("earlier reason"));
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
fn mg8_starts_after_successors_are_reported_untouched() {
    let snapshot = snapshot(&[
        ("src.md", "- [ ] #task Source ^src\n"),
        ("dest.md", "- [ ] #task Target ^dst\n"),
        (
            "other.md",
            "- [ ] #task Other ^other\n\t- 🪢 **LINKS:** starts after [[src#^src]]\n",
        ),
    ]);
    let (staged, reports) = retire(
        &snapshot,
        &request(&["src:src"], MergeContainer::Task(locator("dest:dst"))),
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
    let snapshot = snapshot(&[
        ("closed.md", "- [x] #task Closed ^closed\n"),
        ("dest.md", "- [ ] #task Target ^dst\n"),
    ]);
    let (outcome, _) = classify(
        &snapshot,
        "closed:closed",
        MergeContainer::Task(locator("dest:dst")),
        "merged into",
        MergeGesture::Label,
    );
    assert_eq!(outcome, SourceOutcome::AlreadyClosed);
    let error = retire(
        &snapshot,
        &request(
            &["closed:closed"],
            MergeContainer::Task(locator("dest:dst")),
        ),
    )
    .expect_err("explicit closed source must refuse");
    assert_eq!(error.code, "merge_source_closed");
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
fn mg18_archive_planner_repairs_merged_from_to_tombstone_archive() {
    let vault = tempfile::tempdir().expect("temporary vault");
    let source = "- [-] #task Source [created:: 2026-10-20] [cancelled:: 2026-11-03] ^src\n\t- 🪢 **LINKS:** merged into [[dest#^dst]]\n";
    let target_note = "- [ ] #task Target ^dst\n\t- 🪢 **LINKED FROM:** [[src#^src]] (merged into)\n";
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
            "MG3 relocated collision",
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

    // Exercise source ordering, same-note cuts, cross-note cuts, and both
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

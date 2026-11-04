use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use chrono::NaiveDate;

use super::*;
use crate::native::{
    capture_language::DrawerSelector, note_tasks,
    task_complete::SuccessorSelection, task_locator::TaskLocator,
    task_move::VaultSnapshot, vault_links::NoteIndex,
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

fn post<'a>(plan: &'a PreparedMerge, path: &str) -> &'a str {
    plan.postimages
        .get(Path::new(path))
        .map(String::as_str)
        .unwrap_or_else(|| panic!("missing postimage {path}"))
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

//! Keep-moving engine tests: LP1–LP12 leafness and pure KM vectors
//! (`docs/task-dependencies.md` §12.15–§12.17).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::super::super::pomodoro_homes::{HomeResolver, IndexedHomeSource};
use super::super::super::vault_links::NoteIndex;
use super::super::keep_moving::{
    is_leaf_project, keep_moving_needs_dependents, keep_moving_pre_gate,
    CountingHomeSource, KEEP_MOVING_TRIGGER,
};
use super::super::successors::{
    plan_successors, ClosingContext, ClosingContinuation, CompletedTask,
    EntryRole, OfferGroup, SuccessorInput, SuccessorPlan, SuccessorSelection,
};
use super::*;

fn today() -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(2026, 10, 5).expect("date")
}

fn pred(path: &str, block: &str, id: &str, text: &str) -> CompletedTask {
    CompletedTask {
        relative_path: PathBuf::from(path),
        block_id: block.to_string(),
        task_id: id.to_string(),
        text: text.to_string(),
        is_root: true,
    }
}

fn index(paths: &[&str]) -> NoteIndex {
    NoteIndex::from_paths(paths.iter().map(PathBuf::from))
}

fn area(parent: Option<&str>) -> String {
    match parent {
        Some(parent) => {
            format!("---\ntype: [[area]]\nparent: [[{parent}]]\n---\n")
        }
        None => "---\ntype: [[area]]\n---\n".to_string(),
    }
}

fn project(parent: Option<&str>, extra: &str, title: &str) -> String {
    let parent_line = parent
        .map(|parent| format!("parent: [[{parent}]]\n"))
        .unwrap_or_default();
    format!(
        "---\ntype: [[project]]\n{parent_line}{extra}status: wip\n---\n- [ ] #task #prj {title} ^prj\n"
    )
}

fn leaf_tree() -> BTreeMap<PathBuf, String> {
    let mut notes = BTreeMap::new();
    notes.insert(PathBuf::from("dev.md"), area(None));
    notes.insert(PathBuf::from("sase.md"), project(Some("dev"), "", "Sase"));
    notes.insert(
        PathBuf::from("sase_badges.md"),
        project(Some("sase"), "", "Badges"),
    );
    notes
}

fn badges_body(extra: &str) -> String {
    format!(
        concat!(
            "---\ntype: [[project]]\nparent: [[sase]]\nstatus: wip\n---\n",
            "- [x] #task Draft the badge spec [id:: t] ^t\n",
            "- [ ] #task #prj Badges ^prj\n",
            "{extra}"
        ),
        extra = extra
    )
}

fn residents_pba() -> &'static str {
    concat!(
        "- [/] #task Ship the badge renderer [id:: p] ^p\n",
        "- [*] #task Wire badges into the README [id:: b] ^b\n",
        "- [ ] #task Write badge docs [id:: a] ^a\n",
    )
}

fn day_link(target: &str) -> String {
    format!("## Pomodoros\n- [ ] () \u{2014} FIX\n  - [[{target}]]\n")
}

fn run_km(
    completed: Vec<CompletedTask>,
    snapshot: &[(&str, &str)],
    pre_day: Option<&str>,
    paths: &[&str],
    selection: SuccessorSelection,
) -> SuccessorPlan {
    run_km_full(
        completed, snapshot, pre_day, None, None, paths, true, true, true,
        selection,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_km_full(
    completed: Vec<CompletedTask>,
    snapshot: &[(&str, &str)],
    pre_day: Option<&str>,
    post_day: Option<&str>,
    closing: Option<ClosingContext>,
    paths: &[&str],
    link_unblocked: bool,
    keep_moving: bool,
    snapshot_complete: bool,
    selection: SuccessorSelection,
) -> SuccessorPlan {
    let settings = test_settings();
    let vault_index = index(paths);
    plan_successors(SuccessorInput {
        bob_dir: None,
        completed,
        snapshot: snapshot
            .iter()
            .map(|(path, contents)| (PathBuf::from(path), contents.to_string()))
            .collect(),
        note_texts: snapshot
            .iter()
            .map(|(path, contents)| (PathBuf::from(path), contents.to_string()))
            .collect(),
        pre_day: pre_day.map(str::to_string),
        post_day: post_day.map(str::to_string),
        closing,
        day_relative: PathBuf::from("20261005.md"),
        today: today(),
        tasks_settings: &settings,
        note_settings: &settings,
        link_unblocked,
        keep_moving,
        snapshot_complete,
        selection,
        index: &vault_index,
    })
}

fn leaf_snapshot<'a>(
    badges: &'a str,
    tree: &'a BTreeMap<PathBuf, String>,
) -> Vec<(&'a str, &'a str)> {
    vec![
        ("dev.md", tree[Path::new("dev.md")].as_str()),
        ("sase.md", tree[Path::new("sase.md")].as_str()),
        ("sase_badges.md", badges),
    ]
}

fn leaf_paths() -> &'static [&'static str] {
    &["dev.md", "sase.md", "sase_badges.md", "20261005.md"]
}

fn km_keys(plan: &SuccessorPlan) -> Vec<&str> {
    plan.keep_moving
        .iter()
        .map(|row| row.block_id.as_str())
        .collect()
}

fn lp_notes() -> BTreeMap<PathBuf, String> {
    let mut notes = leaf_tree();
    notes.insert(
        PathBuf::from("sase_goals.md"),
        project(Some("sase"), "", "Goals"),
    );
    notes.insert(
        PathBuf::from("sase_blog.md"),
        project(Some("sase"), "root: true\n", "Blog"),
    );
    notes.insert(
        PathBuf::from("sase_pre_blog.md"),
        project(Some("sase_blog"), "", "Pre blog"),
    );
    notes.insert(PathBuf::from("gtd.md"), area(None));
    notes.insert(PathBuf::from("gtd_daily.md"), area(Some("gtd")));
    notes.insert(PathBuf::from("p.md"), project(Some("gtd_daily"), "", "P"));
    notes.insert(PathBuf::from("o.md"), project(None, "", "Orphan"));
    notes.insert(
        PathBuf::from("missing_parent.md"),
        project(Some("no_such_note"), "", "Missing"),
    );
    notes.insert(
        PathBuf::from("wrapper.md"),
        "---\ntype: [[note]]\nparent: [[sase]]\n---\n".to_string(),
    );
    notes.insert(PathBuf::from("k.md"), project(Some("wrapper"), "", "K"));
    notes.insert(PathBuf::from("m.md"), project(Some("sase"), "", "Middle"));
    notes.insert(PathBuf::from("m_child.md"), project(Some("m"), "", "Child"));
    notes.insert(
        PathBuf::from("inbox.md"),
        "---\ntype: [[area]]\n---\n".to_string(),
    );
    notes.insert(
        PathBuf::from("2026/20261005.md"),
        "---\n---\n- [ ] #task Daily ^d\n".to_string(),
    );
    notes.insert(
        PathBuf::from("done/archived.md"),
        project(Some("sase"), "", "Archived"),
    );
    notes.insert(
        PathBuf::from("terminal.md"),
        "---\ntype: [[project]]\nparent: [[sase]]\nstatus: done\n---\n- [x] #task #prj Done ^prj\n"
            .to_string(),
    );
    notes.insert(
        PathBuf::from("cycle_a.md"),
        project(Some("cycle_b"), "", "A"),
    );
    notes.insert(
        PathBuf::from("cycle_b.md"),
        project(Some("cycle_a"), "", "B"),
    );
    notes
}

fn assert_leaf(
    notes: &BTreeMap<PathBuf, String>,
    path: &str,
    expected: bool,
    vector: &str,
) {
    let index = NoteIndex::from_paths(notes.keys().cloned());
    let mut resolver = HomeResolver::new(IndexedHomeSource::new(&index, notes));
    let contents = notes
        .get(Path::new(path))
        .unwrap_or_else(|| panic!("{vector}: missing {path}"));
    assert_eq!(
        is_leaf_project(
            &mut resolver,
            Path::new(path),
            contents,
            today(),
            &index
        ),
        expected,
        "{vector}: {path}"
    );
}

// LP1–LP12: leafness via HomeResolver, in-memory HomeSource.
#[test]
fn lp_vectors_match_r2_walk() {
    let notes = lp_notes();
    assert_leaf(&notes, "sase_goals.md", true, "LP1");
    assert_leaf(&notes, "sase.md", false, "LP2");
    assert_leaf(&notes, "sase_blog.md", false, "LP3");
    assert_leaf(&notes, "sase_pre_blog.md", true, "LP4");
    assert_leaf(&notes, "p.md", false, "LP5");
    assert_leaf(&notes, "o.md", false, "LP6");
    assert_leaf(&notes, "missing_parent.md", false, "LP7");
    assert_leaf(&notes, "k.md", true, "LP8");
    assert_leaf(&notes, "m.md", true, "LP9");
    assert_leaf(&notes, "sase_goals.md", true, "LP10");
    assert_leaf(&notes, "dev.md", false, "LP11 area");
    assert_leaf(&notes, "inbox.md", false, "LP11 inbox");
    assert_leaf(&notes, "2026/20261005.md", false, "LP11 daily");
    assert_leaf(&notes, "done/archived.md", false, "LP11 done/");
    assert_leaf(&notes, "terminal.md", false, "LP11 terminal");
    assert_leaf(&notes, "cycle_a.md", true, "LP12");
    assert_leaf(&notes, "cycle_b.md", true, "LP12");
}

// LP10: a task `[root::]` field does not change its note's leafness.
#[test]
fn lp10_task_root_field_does_not_change_leafness() {
    let mut notes = lp_notes();
    notes.insert(
        PathBuf::from("sase_goals.md"),
        concat!(
            "---\ntype: [[project]]\nparent: [[sase]]\nstatus: wip\n---\n",
            "- [ ] #task #prj Goals ^prj\n",
            "- [ ] #task With root [root:: sase_blog] [id:: r] ^r\n",
        )
        .to_string(),
    );
    assert_leaf(&notes, "sase_goals.md", true, "LP10");
}

// Cheap G1–G3 gate: no HomeSource, so no ancestor reads.
#[test]
fn g1_g3_pre_gate_is_pure_held_text() {
    let settings = test_settings();
    let badges = badges_body(residents_pba());
    let notes = [(PathBuf::from("sase_badges.md"), badges)];
    let completed =
        vec![pred("sase_badges.md", "t", "t", "Draft the badge spec")];
    let day = day_link("sase_badges#^t");
    assert!(keep_moving_pre_gate(
        &completed,
        &notes,
        Some(&day),
        today(),
        &settings,
    ));
    let non_project = [(
        PathBuf::from("plain.md"),
        "- [x] #task Done [id:: t] ^t\n- [ ] #task Next [id:: n] ^n\n"
            .to_string(),
    )];
    assert!(!keep_moving_pre_gate(
        &[pred("plain.md", "t", "t", "Done")],
        &non_project,
        Some(&day_link("plain#^t")),
        today(),
        &settings,
    ));
    assert!(!keep_moving_pre_gate(
        &completed,
        &notes,
        Some("## Pomodoros\n- [ ] () — FIX\n"),
        today(),
        &settings,
    ));
}

// KM32 / §12.15 "Prerequisite checks": only a dependency-bearing open task
// (a candidate or the `^prj`) needs the vault-wide snapshot proof; a
// triggering close with none builds no snapshot.
#[test]
fn needs_dependents_only_for_dependency_bearing_open_tasks() {
    let settings = test_settings();
    let completed =
        vec![pred("sase_badges.md", "t", "t", "Draft the badge spec")];
    let plain = [(
        PathBuf::from("sase_badges.md"),
        badges_body(residents_pba()),
    )];
    assert!(!keep_moving_needs_dependents(&completed, &plain, &settings));

    let candidate_dep = [(
        PathBuf::from("sase_badges.md"),
        badges_body("- [ ] #task Needs proof [dependsOn:: z] [id:: pr] ^pr\n"),
    )];
    assert!(keep_moving_needs_dependents(
        &completed,
        &candidate_dep,
        &settings,
    ));

    // A closed task's old dependency is no proof burden, and a note that
    // holds no completed task is not consulted.
    let closed_dep = [(
        PathBuf::from("sase_badges.md"),
        badges_body("- [x] #task Old [dependsOn:: z] [id:: pr] ^pr\n"),
    )];
    assert!(!keep_moving_needs_dependents(
        &completed,
        &closed_dep,
        &settings
    ));
    let elsewhere = [(
        PathBuf::from("other.md"),
        "- [ ] #task Needs proof [dependsOn:: z] [id:: pr] ^pr\n".to_string(),
    )];
    assert!(!keep_moving_needs_dependents(
        &completed, &elsewhere, &settings
    ));
}

// Counting source: leaf walk reads only reached ancestors, not the vault.
#[test]
fn leaf_walk_reads_only_reached_ancestors() {
    let notes = lp_notes();
    let index = NoteIndex::from_paths(notes.keys().cloned());
    let source = CountingHomeSource::new(&index, &notes);
    let mut resolver = HomeResolver::new(source);
    assert!(is_leaf_project(
        &mut resolver,
        Path::new("sase_badges.md"),
        &notes[Path::new("sase_badges.md")],
        today(),
        &index,
    ));
    // sase_badges → sase → dev, not the rest of the fixture.
    assert_eq!(resolver.source_mut().reads.get(), 3);
}

// KM1: lane order Pending → Next → Ready; trigger last_planned; unpicked
// writes nothing; pick Ready mints, stamps, links, raises to Next.
#[test]
fn km1_lane_order_unpicked_and_pick() {
    let tree = leaf_tree();
    let badges = badges_body(residents_pba());
    let snapshot = leaf_snapshot(&badges, &tree);
    let day = day_link("sase_badges#^t");
    let completed =
        vec![pred("sase_badges.md", "t", "t", "Draft the badge spec")];
    let none = run_km(
        completed.clone(),
        &snapshot,
        Some(&day),
        leaf_paths(),
        SuccessorSelection::None,
    );
    assert_eq!(km_keys(&none), ["p", "b", "a"], "KM1 lane order");
    for row in &none.keep_moving {
        assert_eq!(row.trigger, KEEP_MOVING_TRIGGER);
        assert_eq!(row.not_linked, Some("not_selected"));
        assert_eq!(row.unblocked_by.len(), 1);
        assert_eq!(row.unblocked_by[0].block_id, "t");
        let offer = row.offer.as_ref().expect("offer");
        assert_eq!(offer.group, OfferGroup::KeepMoving);
        assert_eq!(offer.destination.entry_role, EntryRole::Slot);
        assert!(!offer.selected);
    }
    assert!(none.unblocked.is_empty());
    assert!(
        none.changed_files.is_empty(),
        "KM1/KM25 unpicked writes nothing"
    );
    assert!(none.new_day_text.is_none());
    assert_eq!(none.keep_moving[0].status_symbol, '/');
    assert_eq!(none.keep_moving[1].status_symbol, '*');
    assert_eq!(none.keep_moving[2].status_symbol, ' ');

    let mut keys = BTreeSet::new();
    keys.insert("sase_badges.md#^a".to_string());
    let picked = run_km(
        completed,
        &snapshot,
        Some(&day),
        leaf_paths(),
        SuccessorSelection::Keys(keys),
    );
    assert_eq!(picked.keep_moving.len(), 3);
    let a = picked
        .keep_moving
        .iter()
        .find(|row| row.block_id == "a")
        .expect("picked A");
    assert_eq!(a.previous_status_symbol, ' ');
    assert_eq!(a.status_symbol, '*');
    assert_eq!(a.not_linked, None);
    let link = a.link.as_ref().expect("linked");
    assert!(link.block_link.contains("sase_badges#^a"));
    let updated = picked
        .changed_files
        .get(&PathBuf::from("sase_badges.md"))
        .expect("note edited");
    assert!(updated.contains("[fresh:: 2026-10-05]"));
    assert!(updated.contains("- [*] #task Write badge docs"));
    assert!(updated.contains("- [/] #task Ship the badge renderer"));
    assert!(updated.contains("- [*] #task Wire badges into the README"));
    let day_text = picked.new_day_text.expect("day linked");
    assert!(day_text.contains("[[sase_badges#^a]]"));
}

// KM2 / KM10 / KM12: another open N resident live-linked on the baseline.
#[test]
fn km2_km10_km12_other_live_link_suppresses() {
    let tree = leaf_tree();
    let badges = badges_body(residents_pba());
    let snapshot = leaf_snapshot(&badges, &tree);
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] () \u{2014} FIX\n",
        "  - [[sase_badges#^t]]\n",
        "  - [[sase_badges#^a]]\n",
    );
    let plan = run_km(
        vec![pred("sase_badges.md", "t", "t", "Draft the badge spec")],
        &snapshot,
        Some(day),
        leaf_paths(),
        SuccessorSelection::None,
    );
    assert!(plan.keep_moving.is_empty(), "KM2/KM10/KM12: {plan:?}");
}

// KM3: live link to an already-Done task does not count.
#[test]
fn km3_closed_target_live_link_still_offers() {
    let tree = leaf_tree();
    let badges = badges_body(&format!(
        "- [x] #task Already done [id:: u] ^u\n{}",
        residents_pba()
    ));
    let snapshot = leaf_snapshot(&badges, &tree);
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] () \u{2014} FIX\n",
        "  - [[sase_badges#^t]]\n",
        "  - [[sase_badges#^u]]\n",
    );
    let plan = run_km(
        vec![pred("sase_badges.md", "t", "t", "Draft the badge spec")],
        &snapshot,
        Some(day),
        leaf_paths(),
        SuccessorSelection::None,
    );
    assert_eq!(km_keys(&plan), ["p", "b", "a"], "KM3");
}

// KM4: struck or closed-entry links are not live.
#[test]
fn km4_struck_or_closed_entry_still_offers() {
    let tree = leaf_tree();
    let badges = badges_body(residents_pba());
    let snapshot = leaf_snapshot(&badges, &tree);
    let day = concat!(
        "## Pomodoros\n",
        "- [x] () \u{2014} OLD\n",
        "  - [[sase_badges#^a]]\n",
        "- [ ] () \u{2014} FIX\n",
        "  - [[sase_badges#^t]]\n",
        "  - ~~[[sase_badges#^b]]~~\n",
    );
    let plan = run_km(
        vec![pred("sase_badges.md", "t", "t", "Draft the badge spec")],
        &snapshot,
        Some(day),
        leaf_paths(),
        SuccessorSelection::None,
    );
    assert_eq!(km_keys(&plan), ["p", "b", "a"], "KM4");
}

// KM5: R itself live-linked today suppresses the group.
#[test]
fn km5_prj_live_link_suppresses() {
    let tree = leaf_tree();
    let badges = badges_body(residents_pba());
    let snapshot = leaf_snapshot(&badges, &tree);
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] () \u{2014} FIX\n",
        "  - [[sase_badges#^t]]\n",
        "  - [[sase_badges#^prj]]\n",
    );
    let plan = run_km(
        vec![pred("sase_badges.md", "t", "t", "Draft the badge spec")],
        &snapshot,
        Some(day),
        leaf_paths(),
        SuccessorSelection::None,
    );
    assert!(plan.keep_moving.is_empty(), "KM5");
}

// KM6: top-level / root: true / orphan never keep-moving.
#[test]
fn km6_non_leaf_notes_do_not_trigger() {
    let tree = leaf_tree();
    let sase = concat!(
        "---\ntype: [[project]]\nparent: [[dev]]\nstatus: wip\n---\n",
        "- [x] #task Top [id:: t] ^t\n",
        "- [ ] #task #prj Sase ^prj\n",
        "- [ ] #task Ready [id:: a] ^a\n",
    );
    let snapshot = [
        ("dev.md", tree[Path::new("dev.md")].as_str()),
        ("sase.md", sase),
    ];
    let plan = run_km(
        vec![pred("sase.md", "t", "t", "Top")],
        &snapshot,
        Some(&day_link("sase#^t")),
        &["dev.md", "sase.md", "20261005.md"],
        SuccessorSelection::None,
    );
    assert!(plan.keep_moving.is_empty(), "KM6 LP2");
}

// KM7: middle project offers only its own residents.
#[test]
fn km7_middle_project_lists_own_residents_only() {
    let tree = leaf_tree();
    let badges = badges_body(residents_pba());
    let child = format!(
        "{}\n- [ ] #task Child ready [id:: k] ^k\n",
        project(Some("sase_badges"), "", "Child").trim_end()
    );
    let mut snapshot = leaf_snapshot(&badges, &tree);
    snapshot.push(("sase_badges_child.md", child.as_str()));
    let mut paths = leaf_paths().to_vec();
    paths.push("sase_badges_child.md");
    let plan = run_km(
        vec![pred("sase_badges.md", "t", "t", "Draft the badge spec")],
        &snapshot,
        Some(&day_link("sase_badges#^t")),
        &paths,
        SuccessorSelection::None,
    );
    assert_eq!(km_keys(&plan), ["p", "b", "a"], "KM7");
    assert!(!km_keys(&plan).contains(&"k"));
}

// KM8: predecessor with no own live link does not trigger.
#[test]
fn km8_unplanned_predecessor_does_not_trigger() {
    let tree = leaf_tree();
    let badges = badges_body(residents_pba());
    let snapshot = leaf_snapshot(&badges, &tree);
    let plan = run_km(
        vec![pred("sase_badges.md", "t", "t", "Draft the badge spec")],
        &snapshot,
        Some("## Pomodoros\n- [ ] () \u{2014} FIX\n  - [[sase#^other]]\n"),
        leaf_paths(),
        SuccessorSelection::None,
    );
    assert!(plan.keep_moving.is_empty(), "KM8");
}

// KM9: planned `()` placeholder is a live open entry.
#[test]
fn km9_placeholder_entry_is_a_valid_anchor() {
    let tree = leaf_tree();
    let badges = badges_body(residents_pba());
    let snapshot = leaf_snapshot(&badges, &tree);
    let plan = run_km(
        vec![pred("sase_badges.md", "t", "t", "Draft the badge spec")],
        &snapshot,
        Some(&day_link("sase_badges#^t")),
        leaf_paths(),
        SuccessorSelection::None,
    );
    assert_eq!(
        plan.keep_moving[0]
            .offer
            .as_ref()
            .unwrap()
            .destination
            .entry_name,
        "FIX"
    );
    assert_eq!(
        plan.keep_moving[0]
            .offer
            .as_ref()
            .unwrap()
            .destination
            .entry_role,
        EntryRole::Slot
    );
}

// KM11: Pomodoro close uses the closing anchor.
#[test]
fn km11_closing_anchor() {
    let tree = leaf_tree();
    let badges = badges_body(residents_pba());
    let snapshot = leaf_snapshot(&badges, &tree);
    let pre = day_link("sase_badges#^t");
    let post = concat!(
        "## Pomodoros\n",
        "- [x] () \u{2014} FIX\n",
        "  - [[sase_badges#^t]]\n",
        "- [ ] () \u{2014} FIX\n",
    );
    let plan = run_km_full(
        vec![pred("sase_badges.md", "t", "t", "Draft the badge spec")],
        &snapshot,
        Some(&pre),
        Some(post),
        Some(ClosingContext {
            continuation_name: "FIX".to_string(),
            closed_line: 1,
            continuation: Some(ClosingContinuation {
                line: 4,
                created: true,
            }),
        }),
        leaf_paths(),
        true,
        true,
        true,
        SuccessorSelection::None,
    );
    let offer = plan.keep_moving[0].offer.as_ref().expect("offer");
    assert_eq!(offer.destination.entry_name, "FIX");
    assert!(
        offer.destination.entry_role == EntryRole::Closing
            || offer.destination.entry_role == EntryRole::Created,
        "KM11 role {:?}",
        offer.destination.entry_role
    );
}

// KM11 (R6): the closing anchor names and reuses the continuation by the
// shed name, not the closed entry's narrowed name.
#[test]
fn km11_closing_anchor_follows_the_continuation_name() {
    let tree = leaf_tree();
    let badges = badges_body(residents_pba());
    let snapshot = leaf_snapshot(&badges, &tree);
    let pre = day_link("sase_badges#^t");
    let run = |post: &str| {
        run_km_full(
            vec![pred("sase_badges.md", "t", "t", "Draft the badge spec")],
            &snapshot,
            Some(&pre),
            Some(post),
            Some(ClosingContext {
                continuation_name: "BUILD".to_string(),
                closed_line: 1,
                continuation: None,
            }),
            leaf_paths(),
            true,
            true,
            true,
            SuccessorSelection::None,
        )
    };
    // A later open entry named like the continuation is reused.
    let reused = run(concat!(
        "## Pomodoros\n",
        "- [x] () \u{2014} FIX\n",
        "  - [[sase_badges#^t]]\n",
        "- [ ] () \u{2014} BUILD\n",
    ));
    let destination = &reused.keep_moving[0]
        .offer
        .as_ref()
        .expect("offer")
        .destination;
    assert_eq!(destination.entry_name, "BUILD");
    // Without one, the placeholder successors create wears that name.
    let created = run(concat!(
        "## Pomodoros\n",
        "- [x] () \u{2014} FIX\n",
        "  - [[sase_badges#^t]]\n",
    ));
    let destination = &created.keep_moving[0]
        .offer
        .as_ref()
        .expect("offer")
        .destination;
    assert_eq!(destination.entry_name, "BUILD");
}

// KM13: every remaining resident folded → silence, no group.
#[test]
fn km13_all_folded_is_silence() {
    let tree = leaf_tree();
    let badges = badges_body(concat!(
        "- [?] #task Blocked [id:: bl] ^bl\n",
        "- [ ] #task Hidden #hide [id:: hi] ^hi\n",
        "- [ ] #task Later [scheduled:: 2026-10-13] [id:: sc] ^sc\n",
    ));
    let snapshot = leaf_snapshot(&badges, &tree);
    let plan = run_km(
        vec![pred("sase_badges.md", "t", "t", "Draft the badge spec")],
        &snapshot,
        Some(&day_link("sase_badges#^t")),
        leaf_paths(),
        SuccessorSelection::None,
    );
    assert!(plan.keep_moving.is_empty(), "KM13");
    assert!(plan.keep_moving_folds.is_empty());
}

// KM14: last open task is a project_finished pull-in, not keep-moving.
#[test]
fn km14_emptied_project_is_not_keep_moving() {
    let tree = leaf_tree();
    let badges = concat!(
        "---\ntype: [[project]]\nparent: [[sase]]\nstatus: wip\n---\n",
        "- [x] #task Draft the badge spec [id:: t] ^t\n",
        "- [ ] #task #prj Badges #hide ^prj\n",
    );
    let snapshot = leaf_snapshot(badges, &tree);
    let plan = run_km(
        vec![pred("sase_badges.md", "t", "t", "Draft the badge spec")],
        &snapshot,
        Some(&day_link("sase_badges#^t")),
        leaf_paths(),
        SuccessorSelection::None,
    );
    assert!(plan.keep_moving.is_empty(), "KM14 {:?}", plan.keep_moving);
}

// KM15: a direct dependent stays in hand_off.
#[test]
fn km15_direct_dependent_stays_hand_off() {
    let tree = leaf_tree();
    let badges = badges_body(concat!(
        "- [ ] #task Write badge docs [dependsOn:: t] [id:: a] ^a\n",
        "- [/] #task Ship the badge renderer [id:: p] ^p\n",
    ));
    let snapshot = leaf_snapshot(&badges, &tree);
    let plan = run_km(
        vec![pred("sase_badges.md", "t", "t", "Draft the badge spec")],
        &snapshot,
        Some(&day_link("sase_badges#^t")),
        leaf_paths(),
        SuccessorSelection::None,
    );
    assert!(plan.unblocked.iter().any(|row| {
        row.block_id == "a"
            && row
                .offer
                .as_ref()
                .is_some_and(|offer| offer.group == OfferGroup::HandOff)
    }));
    assert_eq!(km_keys(&plan), ["p"]);
}

// KM16: blocked ^prj suppresses keep-moving.
#[test]
fn km16_blocked_root_suppresses() {
    let tree = leaf_tree();
    let badges = concat!(
        "---\ntype: [[project]]\nparent: [[sase]]\nstatus: wip\n---\n",
        "- [x] #task Draft the badge spec [id:: t] ^t\n",
        "- [?] #task #prj Badges [scheduled:: 2026-10-13] ^prj\n",
        "- [ ] #task Write badge docs [id:: a] ^a\n",
    );
    let snapshot = leaf_snapshot(badges, &tree);
    let plan = run_km(
        vec![pred("sase_badges.md", "t", "t", "Draft the badge spec")],
        &snapshot,
        Some(&day_link("sase_badges#^t")),
        leaf_paths(),
        SuccessorSelection::None,
    );
    assert!(plan.keep_moving.is_empty(), "KM16");
}

// KM17: fold blocked / scheduled / hidden; scheduled today is offered.
#[test]
fn km17_folds_and_scheduled_today_offered() {
    let tree = leaf_tree();
    let badges = badges_body(concat!(
        "- [?] #task Blocked mark [id:: q] ^q\n",
        "- [ ] #task Waits [dependsOn:: open-x] [id:: w] ^w\n",
        "- [ ] #task Tomorrow [scheduled:: 2026-10-06] [id:: tm] ^tm\n",
        "- [ ] #task Hidden #hide [id:: hi] ^hi\n",
        "- [ ] #task Due today [scheduled:: 2026-10-05] [id:: td] ^td\n",
        "- [ ] #task Ready [id:: a] ^a\n",
        "- [ ] #task Open x [id:: open-x] ^open-x\n",
    ));
    let snapshot = leaf_snapshot(&badges, &tree);
    let plan = run_km(
        vec![pred("sase_badges.md", "t", "t", "Draft the badge spec")],
        &snapshot,
        Some(&day_link("sase_badges#^t")),
        leaf_paths(),
        SuccessorSelection::None,
    );
    assert_eq!(km_keys(&plan), ["td", "a", "open-x"]);
    let fold = &plan.keep_moving_folds[0];
    assert_eq!(fold.blocked, 2, "KM17 blocked");
    assert_eq!(fold.scheduled, 1, "KM17 scheduled");
    assert_eq!(fold.hidden, 1, "KM17 hidden");
}

// KM18: #ref, nested subtask, recurring due today.
#[test]
fn km18_ref_nested_recurring_are_offered() {
    let tree = leaf_tree();
    let badges = badges_body(concat!(
        "- [ ] #task #ref Read the paper [id:: rf] ^rf\n",
        "- [ ] #task Parent [id:: nest] ^nest\n",
        "  - [ ] #task Nested [id:: nested] ^nested\n",
        "- [ ] #task Water [repeat:: weekly] [id:: rec] ^rec\n",
    ));
    let snapshot = leaf_snapshot(&badges, &tree);
    let plan = run_km(
        vec![pred("sase_badges.md", "t", "t", "Draft the badge spec")],
        &snapshot,
        Some(&day_link("sase_badges#^t")),
        leaf_paths(),
        SuccessorSelection::None,
    );
    let keys = km_keys(&plan);
    assert!(keys.contains(&"rf"), "KM18 ref {keys:?}");
    assert!(keys.contains(&"nested"), "KM18 nested {keys:?}");
    assert!(keys.contains(&"rec"), "KM18 recurring {keys:?}");
}

// KM19: both kill switches.
#[test]
fn km19_kill_switches() {
    let tree = leaf_tree();
    let badges = badges_body(residents_pba());
    let snapshot = leaf_snapshot(&badges, &tree);
    let day = day_link("sase_badges#^t");
    let completed =
        vec![pred("sase_badges.md", "t", "t", "Draft the badge spec")];
    let off = run_km_full(
        completed.clone(),
        &snapshot,
        Some(&day),
        None,
        None,
        leaf_paths(),
        true,
        false,
        true,
        SuccessorSelection::None,
    );
    assert!(off.keep_moving.is_empty(), "KM19 keep_moving false");
    let disabled = run_km_full(
        completed,
        &snapshot,
        Some(&day),
        None,
        None,
        leaf_paths(),
        false,
        true,
        true,
        SuccessorSelection::None,
    );
    assert!(disabled.keep_moving.is_empty(), "KM19 link_unblocked false");
}

// KM22: two leaf projects ordered by anchor then path.
#[test]
fn km22_two_projects_order_by_anchor_then_path() {
    let tree = leaf_tree();
    let badges = badges_body("- [ ] #task Badge ready [id:: a] ^a\n");
    let goals = concat!(
        "---\ntype: [[project]]\nparent: [[sase]]\nstatus: wip\n---\n",
        "- [x] #task Goal pred [id:: g] ^g\n",
        "- [ ] #task #prj Goals ^prj\n",
        "- [ ] #task Goal ready [id:: ga] ^ga\n",
    );
    let snapshot = [
        ("dev.md", tree[Path::new("dev.md")].as_str()),
        ("sase.md", tree[Path::new("sase.md")].as_str()),
        ("sase_badges.md", badges.as_str()),
        ("sase_goals.md", goals),
    ];
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] () \u{2014} FIX\n",
        "  - [[sase_badges#^t]]\n",
        "  - [[sase_goals#^g]]\n",
    );
    let plan = run_km(
        vec![
            pred("sase_badges.md", "t", "t", "Draft the badge spec"),
            pred("sase_goals.md", "g", "g", "Goal pred"),
        ],
        &snapshot,
        Some(day),
        &[
            "dev.md",
            "sase.md",
            "sase_badges.md",
            "sase_goals.md",
            "20261005.md",
        ],
        SuccessorSelection::None,
    );
    assert_eq!(km_keys(&plan), ["a", "ga"], "KM22 {:?}", km_keys(&plan));
}

// KM23: `[root::]` on T does not change leafness.
#[test]
fn km23_predecessor_root_field_still_triggers() {
    let tree = leaf_tree();
    let badges = concat!(
        "---\ntype: [[project]]\nparent: [[sase]]\nstatus: wip\n---\n",
        "- [x] #task Draft the badge spec [root:: sase] [id:: t] ^t\n",
        "- [ ] #task #prj Badges ^prj\n",
        "- [ ] #task Write badge docs [id:: a] ^a\n",
    );
    let snapshot = leaf_snapshot(badges, &tree);
    let plan = run_km(
        vec![pred("sase_badges.md", "t", "t", "Draft the badge spec")],
        &snapshot,
        Some(&day_link("sase_badges#^t")),
        leaf_paths(),
        SuccessorSelection::None,
    );
    assert_eq!(km_keys(&plan), ["a"], "KM23");
}

// KM25: `-p none` writes nothing, including Pending preservation on unpicked.
#[test]
fn km25_none_writes_nothing() {
    let tree = leaf_tree();
    let original = badges_body(residents_pba());
    let snapshot = leaf_snapshot(&original, &tree);
    let plan = run_km(
        vec![pred("sase_badges.md", "t", "t", "Draft the badge spec")],
        &snapshot,
        Some(&day_link("sase_badges#^t")),
        leaf_paths(),
        SuccessorSelection::None,
    );
    assert!(plan.changed_files.is_empty());
    assert!(plan.new_day_text.is_none());
    assert!(plan.keep_moving.iter().all(|row| row.link.is_none()));
}

// KM29: completing R itself never keep-moves.
#[test]
fn km29_closing_prj_does_not_keep_moving() {
    let tree = leaf_tree();
    let badges = concat!(
        "---\ntype: [[project]]\nparent: [[sase]]\nstatus: wip\n---\n",
        "- [x] #task #prj Badges [id:: prj] ^prj\n",
        "- [ ] #task Write badge docs [id:: a] ^a\n",
    );
    let snapshot = leaf_snapshot(badges, &tree);
    let plan = run_km(
        vec![pred("sase_badges.md", "prj", "prj", "Badges")],
        &snapshot,
        Some(&day_link("sase_badges#^prj")),
        leaf_paths(),
        SuccessorSelection::None,
    );
    assert!(plan.keep_moving.is_empty(), "KM29");
}

// KM31: unavailable snapshot folds candidates with dependsOn; others remain.
#[test]
fn km31_unavailable_snapshot_folds_dependency_bearers() {
    let tree = leaf_tree();
    let badges = badges_body(concat!(
        "- [ ] #task Waits [dependsOn:: other] [id:: w] ^w\n",
        "- [ ] #task Ready [id:: a] ^a\n",
    ));
    let snapshot = leaf_snapshot(&badges, &tree);
    let plan = run_km_full(
        vec![pred("sase_badges.md", "t", "t", "Draft the badge spec")],
        &snapshot,
        Some(&day_link("sase_badges#^t")),
        None,
        None,
        leaf_paths(),
        true,
        true,
        false,
        SuccessorSelection::None,
    );
    assert_eq!(km_keys(&plan), ["a"], "KM31 {:?}", km_keys(&plan));
    assert_eq!(plan.keep_moving_folds[0].blocked, 1);
}

// KM31: unsafe project root with unproved dependsOn suppresses the group.
#[test]
fn km31_unproved_root_deps_suppress_group() {
    let tree = leaf_tree();
    let badges = concat!(
        "---\ntype: [[project]]\nparent: [[sase]]\nstatus: wip\n---\n",
        "- [x] #task Draft the badge spec [id:: t] ^t\n",
        "- [ ] #task #prj Badges [dependsOn:: other] ^prj\n",
        "- [ ] #task Ready [id:: a] ^a\n",
    );
    let snapshot = leaf_snapshot(badges, &tree);
    let plan = run_km_full(
        vec![pred("sase_badges.md", "t", "t", "Draft the badge spec")],
        &snapshot,
        Some(&day_link("sase_badges#^t")),
        None,
        None,
        leaf_paths(),
        true,
        true,
        false,
        SuccessorSelection::None,
    );
    assert!(plan.keep_moving.is_empty(), "KM31 root");
}

// KM32: close outside a project note adds no keep-moving and needs no
// ancestor notes in the snapshot.
#[test]
fn km32_non_project_close_is_quiet() {
    let plan = run_km(
        vec![pred("plain.md", "t", "t", "Done")],
        &[(
            "plain.md",
            "- [x] #task Done [id:: t] ^t\n- [ ] #task Next [id:: n] ^n\n",
        )],
        Some(&day_link("plain#^t")),
        &["plain.md", "20261005.md"],
        SuccessorSelection::None,
    );
    assert!(plan.keep_moving.is_empty(), "KM32");
}

// Mint-needed Ready pick; Pending/Next status preserved on a pick-all.
#[test]
fn pick_preserves_pending_and_mints_idless_ready() {
    let tree = leaf_tree();
    let badges = concat!(
        "---\ntype: [[project]]\nparent: [[sase]]\nstatus: wip\n---\n",
        "- [x] #task Draft the badge spec [id:: t] ^t\n",
        "- [ ] #task #prj Badges ^prj\n",
        "- [/] #task Ship it [id:: p] ^p\n",
        "- [ ] #task Docs without id\n",
    );
    let snapshot = leaf_snapshot(badges, &tree);
    let plan = run_km(
        vec![pred("sase_badges.md", "t", "t", "Draft the badge spec")],
        &snapshot,
        Some(&day_link("sase_badges#^t")),
        leaf_paths(),
        SuccessorSelection::All,
    );
    let pending = plan
        .keep_moving
        .iter()
        .find(|row| row.block_id == "p")
        .expect("pending");
    assert_eq!(pending.status_symbol, '/');
    let docs = plan
        .keep_moving
        .iter()
        .find(|row| row.text.contains("Docs without id"))
        .expect("docs");
    assert_eq!(docs.status_symbol, '*');
    assert!(docs.link.as_ref().expect("link").block_id_created);
    let updated = plan
        .changed_files
        .get(&PathBuf::from("sase_badges.md"))
        .expect("edited");
    assert!(updated.contains("[fresh:: 2026-10-05]"));
    assert!(updated.contains("- [/] #task Ship it"));
}

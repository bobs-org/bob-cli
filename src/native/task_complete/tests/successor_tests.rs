//! Successor-link planner tests: outcomes of the shared rule
//! (`docs/task-dependencies.md` §11.6 SL vectors) through the pure
//! planner against in-memory snapshots, in the style of
//! `recovery_tests.rs`.

use super::super::super::vault_links::NoteIndex;
use super::super::successors::{
    insert_successor_links, plan_successors, CompletedTask, EntryRole,
    OfferGroup, SuccessorInput, SuccessorPlacement, SuccessorSelection,
    SuccessorTarget,
};
use super::*;

fn today() -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(2026, 10, 5).expect("date")
}

fn pred(
    path: &str,
    block: &str,
    id: &str,
    text: &str,
    is_root: bool,
) -> CompletedTask {
    CompletedTask {
        relative_path: PathBuf::from(path),
        block_id: block.to_string(),
        task_id: id.to_string(),
        text: text.to_string(),
        is_root,
    }
}

fn index(paths: &[&str]) -> NoteIndex {
    NoteIndex::from_paths(paths.iter().map(PathBuf::from))
}

fn run(
    completed: Vec<CompletedTask>,
    snapshot: &[(&str, &str)],
    pre_day: Option<&str>,
    day_relative: &str,
    link_unblocked: bool,
    index: &NoteIndex,
) -> super::super::successors::SuccessorPlan {
    run_sel(
        completed,
        snapshot,
        pre_day,
        day_relative,
        link_unblocked,
        index,
        SuccessorSelection::All,
    )
}

fn run_sel(
    completed: Vec<CompletedTask>,
    snapshot: &[(&str, &str)],
    pre_day: Option<&str>,
    day_relative: &str,
    link_unblocked: bool,
    index: &NoteIndex,
    selection: SuccessorSelection,
) -> super::super::successors::SuccessorPlan {
    let settings = test_settings();
    plan_successors(SuccessorInput {
        completed,
        snapshot: snapshot
            .iter()
            .map(|(path, contents)| (PathBuf::from(path), contents.to_string()))
            .collect(),
        note_texts: Vec::new(),
        pre_day: pre_day.map(str::to_string),
        post_day: None,
        closing: None,
        day_relative: PathBuf::from(day_relative),
        today: today(),
        tasks_settings: &settings,
        note_settings: &settings,
        link_unblocked,
        selection,
        index,
    })
}

fn day_with_fix_link() -> String {
    concat!(
        "## Pomodoros\n",
        "- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
        "  - [[sase#^other]]\n",
        "- [ ] () \u{2014} FIX\n",
        "  - [[sase#^fix-apollo]]\n",
    )
    .to_string()
}

// SL1: P linked in the running entry; D `[?]` depends only on P. The
// successor links right after P's bullet subtree and goes `[?]`→`[*]`.
#[test]
fn sl1_links_blocked_dependent_into_slot() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let plan = run(
        vec![pred("sase.md", "fix-apollo", "fix-apollo", "Fix apollo", true)],
        &[(
            "sase.md",
            concat!(
                "- [x] #task Fix apollo [id:: fix-apollo] ^fix-apollo\n",
                "- [?] #task Relaunch [dependsOn:: fix-apollo] [id:: relaunch] ^relaunch\n",
            ),
        )],
        Some(&day_with_fix_link()),
        "20261005.md",
        true,
        &vault_index,
    );
    assert!(plan.ran_lookup);
    assert_eq!(plan.unblocked.len(), 1);
    let row = &plan.unblocked[0];
    assert_eq!(row.previous_status_symbol, '?');
    assert_eq!(row.status_symbol, '*');
    assert_eq!(row.block_id, "relaunch");
    assert_eq!(row.not_linked, None);
    assert_eq!(row.unblocked_by.len(), 1);
    assert_eq!(row.unblocked_by[0].block_id, "fix-apollo");
    let link = row.link.as_ref().expect("linked");
    assert_eq!(link.entry_name, "FIX");
    assert_eq!(link.block_link, "[[sase#^relaunch]]");
    assert!(!link.block_id_created);
    assert!(plan.still_blocked.is_empty());
    let updated = plan
        .changed_files
        .get(&PathBuf::from("sase.md"))
        .expect("sase.md changed");
    assert!(updated.contains(
        "- [*] #task Relaunch [fresh:: 2026-10-05] [dependsOn:: fix-apollo] [id:: relaunch] ^relaunch"
    ));
    let day = plan.new_day_text.expect("day changed");
    assert!(day.contains("  - [[sase#^relaunch]]\n"));
}

// SL3 + SL4: another open prerequisite, or a future schedule, keeps the
// dependent blocked with a `still_blocked` row and no status change.
#[test]
fn sl3_and_sl4_stay_blocked_with_reasons() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let plan = run(
        vec![pred("sase.md", "done-p", "done-p", "Done P", true)],
        &[(
            "sase.md",
            concat!(
                "- [x] #task Done P [id:: done-p] ^done-p\n",
                "- [ ] #task Still open [id:: live-q] ^live-q\n",
                "- [?] #task Waits on both [dependsOn:: done-p, live-q] [id:: both] ^both\n",
                "- [?] #task Future [dependsOn:: done-p] [scheduled:: 2026-10-13] [id:: future] ^future\n",
            ),
        )],
        Some(&day_with_fix_link()),
        "20261005.md",
        true,
        &vault_index,
    );
    assert!(plan.unblocked.is_empty());
    assert_eq!(plan.still_blocked.len(), 2);
    let waits = plan
        .still_blocked
        .iter()
        .find(|row| row.block_id == "both")
        .expect("waits_on row");
    assert_eq!(waits.reason, "waits_on");
    assert_eq!(waits.waits_on, 1);
    let scheduled = plan
        .still_blocked
        .iter()
        .find(|row| row.block_id == "future")
        .expect("scheduled row");
    assert_eq!(scheduled.reason, "scheduled");
    assert_eq!(scheduled.scheduled.as_deref(), Some("2026-10-13"));
    assert!(plan.changed_files.is_empty());
    assert!(plan.new_day_text.is_none());
}

// SL5: D already has a live link under another open entry. No new link
// (`already_planned`), but `[?]` still recovers to `[*]`.
#[test]
fn sl5_already_planned_recovers_without_link() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
        "- [ ] () \u{2014} FIX\n",
        "  - [[sase#^fix-apollo]]\n",
        "- [ ] () \u{2014} SASE\n",
        "  - [[sase#^relaunch]]\n",
    );
    let plan = run(
        vec![pred("sase.md", "fix-apollo", "fix-apollo", "Fix apollo", true)],
        &[(
            "sase.md",
            concat!(
                "- [x] #task Fix apollo [id:: fix-apollo] ^fix-apollo\n",
                "- [?] #task Relaunch [dependsOn:: fix-apollo] [id:: relaunch] ^relaunch\n",
            ),
        )],
        Some(day),
        "20261005.md",
        true,
        &vault_index,
    );
    assert_eq!(plan.unblocked.len(), 1);
    let row = &plan.unblocked[0];
    assert_eq!(row.previous_status_symbol, '?');
    assert_eq!(row.status_symbol, '*');
    assert_eq!(row.link, None);
    assert_eq!(row.not_linked, Some("already_planned"));
    assert!(plan.new_day_text.is_none());
}

// SL6: struck links and links under closed entries are history, not
// plans, so the dependent still links.
#[test]
fn sl6_struck_history_does_not_count_as_planned() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let day = concat!(
        "## Pomodoros\n",
        "- [x] (**0800-0820**) \u{2014} DONE\n",
        "  - [[sase#^relaunch]]\n",
        "- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
        "- [ ] () \u{2014} FIX\n",
        "  - [[sase#^fix-apollo]]\n",
        "  - ~~[[sase#^relaunch]]~~\n",
    );
    let plan = run(
        vec![pred("sase.md", "fix-apollo", "fix-apollo", "Fix apollo", true)],
        &[(
            "sase.md",
            concat!(
                "- [x] #task Fix apollo [id:: fix-apollo] ^fix-apollo\n",
                "- [?] #task Relaunch [dependsOn:: fix-apollo] [id:: relaunch] ^relaunch\n",
            ),
        )],
        Some(day),
        "20261005.md",
        true,
        &vault_index,
    );
    assert_eq!(plan.unblocked.len(), 1);
    assert!(plan.unblocked[0].link.is_some());
}

// SL9 / SO14: with no live link today the dependent is offered at the
// implicit destination (CAPTURE: one open timed entry). Select-all
// links it as an entry child (`fallback: true`).
#[test]
fn sl9_unplanned_predecessor_links_at_implicit_destination() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let plan = run(
        vec![pred("sase.md", "fix-apollo", "fix-apollo", "Fix apollo", true)],
        &[(
            "sase.md",
            concat!(
                "- [x] #task Fix apollo [id:: fix-apollo] ^fix-apollo\n",
                "- [?] #task Relaunch [dependsOn:: fix-apollo] [id:: relaunch] ^relaunch\n",
            ),
        )],
        Some(&day_with_fix_link().replace("[[sase#^fix-apollo]]", "[[sase#^other]]")),
        "20261005.md",
        true,
        &vault_index,
    );
    assert_eq!(plan.unblocked.len(), 1);
    let row = &plan.unblocked[0];
    assert_eq!(row.previous_status_symbol, '?');
    assert_eq!(row.status_symbol, '*');
    assert_eq!(row.not_linked, None);
    let offer = row.offer.as_ref().expect("implicit offer");
    assert_eq!(offer.group, OfferGroup::HandOff);
    assert_eq!(offer.destination.entry_name, "CAPTURE");
    assert_eq!(offer.destination.entry_role, EntryRole::Implicit);
    assert!(offer.destination.fallback);
    assert!(offer.selected);
    let link = row.link.as_ref().expect("linked");
    assert_eq!(link.entry_name, "CAPTURE");
    assert!(!link.entry_created);
    let day = plan.new_day_text.expect("day changed");
    assert!(day.contains("[[sase#^relaunch]]"), "{day}");
}

// SO15: unplanned predecessor with no open entry recovers, no offer.
#[test]
fn so15_unplanned_predecessor_no_open_entry_is_no_destination() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let plan = run(
        vec![pred("sase.md", "fix-apollo", "fix-apollo", "Fix apollo", true)],
        &[(
            "sase.md",
            concat!(
                "- [x] #task Fix apollo [id:: fix-apollo] ^fix-apollo\n",
                "- [?] #task Relaunch [dependsOn:: fix-apollo] [id:: relaunch] ^relaunch\n",
            ),
        )],
        Some("## Pomodoros\n- [x] (**0800-0820**) \u{2014} DONE\n"),
        "20261005.md",
        true,
        &vault_index,
    );
    assert_eq!(plan.unblocked.len(), 1);
    let row = &plan.unblocked[0];
    assert_eq!(row.not_linked, Some("no_destination"));
    assert!(row.offer.is_none());
    assert!(row.link.is_none());
    assert_eq!(row.status_symbol, ' ');
    assert!(plan.new_day_text.is_none());
}

// SO16: unplanned predecessor with two open timed entries recovers,
// no offer.
#[test]
fn so16_unplanned_predecessor_two_open_timed_is_no_destination() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] (**0900-0920**) \u{2014} ONE\n",
        "- [ ] (**0930-0950**) \u{2014} TWO\n",
    );
    let plan = run(
        vec![pred("sase.md", "fix-apollo", "fix-apollo", "Fix apollo", true)],
        &[(
            "sase.md",
            concat!(
                "- [x] #task Fix apollo [id:: fix-apollo] ^fix-apollo\n",
                "- [?] #task Relaunch [dependsOn:: fix-apollo] [id:: relaunch] ^relaunch\n",
            ),
        )],
        Some(day),
        "20261005.md",
        true,
        &vault_index,
    );
    assert_eq!(plan.unblocked.len(), 1);
    let row = &plan.unblocked[0];
    assert_eq!(row.not_linked, Some("no_destination"));
    assert!(row.offer.is_none());
    assert!(row.link.is_none());
    assert!(plan.new_day_text.is_none());
}

// SL10: a successor without a `^block-id` gets one minted (SB rule) and
// appended to its task line.
#[test]
fn sl10_mints_missing_block_id() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let plan = run(
        vec![pred("sase.md", "fix-apollo", "fix-apollo", "Fix apollo", true)],
        &[(
            "sase.md",
            concat!(
                "- [x] #task Fix apollo [id:: fix-apollo] ^fix-apollo\n",
                "- [?] #task Book flights [dependsOn:: fix-apollo] [id:: book-flights]\n",
            ),
        )],
        Some(&day_with_fix_link()),
        "20261005.md",
        true,
        &vault_index,
    );
    assert_eq!(plan.unblocked.len(), 1);
    let row = &plan.unblocked[0];
    assert_eq!(row.block_id, "book-flights");
    let link = row.link.as_ref().expect("linked");
    assert!(link.block_id_created);
    assert_eq!(link.block_link, "[[sase#^book-flights]]");
    let updated = plan
        .changed_files
        .get(&PathBuf::from("sase.md"))
        .expect("sase.md changed");
    assert!(updated.contains("[id:: book-flights] ^book-flights"));
}

// SL11: `^prj` and `#hide` dependents recover only, never link.
#[test]
fn sl11_project_and_hidden_tasks_recover_only() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let plan = run(
        vec![pred("sase.md", "fix-apollo", "fix-apollo", "Fix apollo", true)],
        &[(
            "sase.md",
            concat!(
                "- [x] #task Fix apollo [id:: fix-apollo] ^fix-apollo\n",
                "- [?] #task Child project [dependsOn:: fix-apollo] [id:: child] ^prj\n",
                "- [?] #task Quiet one [dependsOn:: fix-apollo] [id:: quiet] #hide ^quiet\n",
            ),
        )],
        Some(&day_with_fix_link()),
        "20261005.md",
        true,
        &vault_index,
    );
    assert_eq!(plan.unblocked.len(), 2);
    for row in &plan.unblocked {
        assert!(row.link.is_none());
    }
    let reasons = plan
        .unblocked
        .iter()
        .map(|row| (row.block_id.clone(), row.not_linked))
        .collect::<Vec<_>>();
    assert!(reasons.contains(&("prj".to_string(), Some("project_task"))));
    assert!(reasons.contains(&("quiet".to_string(), Some("hidden"))));
    assert!(plan.new_day_text.is_none());
}

// A case variant of `^prj` is an ordinary task. Closing its prerequisite
// recovers and links it as a normal successor (the matcher is exact).
#[test]
fn uppercase_prj_lookalike_links_as_ordinary_successor() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let day =
        concat!("## Pomodoros\n", "- [ ] () — FIX\n", "  - [[sase#^p]]\n",);
    let plan = run(
        vec![pred("sase.md", "p", "p", "Prerequisite", true)],
        &[(
            "sase.md",
            concat!(
                "- [x] #task Prerequisite [id:: p] ^p\n",
                "- [?] #task Ordinary task [dependsOn:: p] [id:: ordinary] ^PRJ\n",
            ),
        )],
        Some(day),
        "20261005.md",
        true,
        &vault_index,
    );
    let row = plan
        .unblocked
        .iter()
        .find(|row| row.block_id == "PRJ")
        .expect("ordinary ^PRJ successor recovered");
    assert_eq!(row.not_linked, None);
    assert!(row.link.is_some());
    assert_eq!(row.status_symbol, '*');
    assert!(plan
        .new_day_text
        .as_deref()
        .is_some_and(|text| text.contains("[[sase#^PRJ]]")));
}

// SL12: one gesture closes P1 and P2; D depends on both. D links once,
// after the earlier anchor, naming both predecessors.
#[test]
fn sl12_two_predecessors_link_once_at_earlier_anchor() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
        "- [ ] () \u{2014} FIX\n",
        "  - [[sase#^p-one]]\n",
        "  - [[sase#^p-two]]\n",
    );
    let plan = run(
        vec![
            pred("sase.md", "p-one", "p-one", "First", true),
            pred("sase.md", "p-two", "p-two", "Second", false),
        ],
        &[(
            "sase.md",
            concat!(
                "- [x] #task First [id:: p-one] ^p-one\n",
                "- [x] #task Second [id:: p-two] ^p-two\n",
                "- [?] #task Joint [dependsOn:: p-one, p-two] [id:: joint] ^joint\n",
            ),
        )],
        Some(day),
        "20261005.md",
        true,
        &vault_index,
    );
    assert_eq!(plan.unblocked.len(), 1);
    let row = &plan.unblocked[0];
    assert!(row.link.is_some());
    assert_eq!(row.unblocked_by.len(), 2);
    let day_text = plan.new_day_text.expect("day changed");
    let joint = day_text.find("[[sase#^joint]]").expect("joint linked");
    let one = day_text.find("[[sase#^p-one]]").expect("p-one anchor");
    assert!(joint > one);
}

// SL13: chains do not recurse; E (depending on D) is untouched and not
// reported. SL15: a stale `[ ]` dependent still links and becomes Next.
#[test]
fn sl13_chain_waits_and_sl15_stale_ready_links() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let plan = run(
        vec![pred("sase.md", "fix-apollo", "fix-apollo", "Fix apollo", true)],
        &[(
            "sase.md",
            concat!(
                "- [x] #task Fix apollo [id:: fix-apollo] ^fix-apollo\n",
                "- [ ] #task Stale ready [dependsOn:: fix-apollo] [id:: stale] ^stale\n",
                "- [?] #task Next hop [dependsOn:: stale] [id:: hop] ^hop\n",
            ),
        )],
        Some(&day_with_fix_link()),
        "20261005.md",
        true,
        &vault_index,
    );
    assert_eq!(plan.unblocked.len(), 1);
    let row = &plan.unblocked[0];
    assert_eq!(row.block_id, "stale");
    assert_eq!(row.previous_status_symbol, ' ');
    assert_eq!(row.status_symbol, '*');
    assert!(plan.still_blocked.is_empty());
}

// SL16: no `[id::]` on any completed task stops the planner with no
// lookup.
#[test]
fn sl16_gate_without_ids_runs_nothing() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let plan = run(
        vec![pred("sase.md", "plain", "", "Plain", true)],
        &[(
            "sase.md",
            "- [x] #task Plain ^plain\n- [?] #task Waiter [dependsOn:: ghost] [id:: waiter] ^waiter\n",
        )],
        Some(&day_with_fix_link()),
        "20261005.md",
        true,
        &vault_index,
    );
    assert!(!plan.ran_lookup);
    assert!(plan.unblocked.is_empty());
    assert!(plan.still_blocked.is_empty());
    assert!(plan.changed_files.is_empty());
}

// SO2: 11-way fan-out. None links nothing (`not_selected`, no mint, no
// day edit); All links all 11. No breaker, no cap.
#[test]
fn so2_eleven_way_fan_out() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let mut note =
        "- [x] #task Fix apollo [id:: fix-apollo] ^fix-apollo\n".to_string();
    for n in 1..=11 {
        note.push_str(&format!(
            "- [?] #task Fan {n} [dependsOn:: fix-apollo] [id:: fan-{n}] ^fan-{n}\n"
        ));
    }
    let none = run_sel(
        vec![pred(
            "sase.md",
            "fix-apollo",
            "fix-apollo",
            "Fix apollo",
            true,
        )],
        &[("sase.md", &note)],
        Some(&day_with_fix_link()),
        "20261005.md",
        true,
        &vault_index,
        SuccessorSelection::None,
    );
    assert_eq!(none.unblocked.len(), 11);
    for row in &none.unblocked {
        assert_eq!(row.not_linked, Some("not_selected"));
        assert!(row.link.is_none());
        let offer = row.offer.as_ref().expect("hand_off offer");
        assert_eq!(offer.group, OfferGroup::HandOff);
        assert!(!offer.selected);
    }
    assert!(none.new_day_text.is_none());
    let all = run_sel(
        vec![pred(
            "sase.md",
            "fix-apollo",
            "fix-apollo",
            "Fix apollo",
            true,
        )],
        &[("sase.md", &note)],
        Some(&day_with_fix_link()),
        "20261005.md",
        true,
        &vault_index,
        SuccessorSelection::All,
    );
    assert_eq!(all.unblocked.len(), 11);
    for row in &all.unblocked {
        assert!(row.link.is_some(), "{row:?}");
        assert_eq!(row.not_linked, None);
        assert_eq!(row.status_symbol, '*');
        assert!(row.offer.as_ref().is_some_and(|offer| offer.selected));
    }
    let day = all.new_day_text.expect("day changed");
    for n in 1..=11 {
        assert!(day.contains(&format!("[[sase#^fan-{n}]]")), "{day}");
    }
}

// SL18 / SO19: `plan.link_unblocked: false` recovers to the derived
// rank with no links, no mints, and no offers (`disabled`).
#[test]
fn sl18_disabled_recovers_without_links_or_mints() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let plan = run(
        vec![pred(
            "sase.md",
            "fix-apollo",
            "fix-apollo",
            "Fix apollo",
            true,
        )],
        &[(
            "sase.md",
            concat!(
                "- [x] #task Fix apollo [id:: fix-apollo] ^fix-apollo\n",
                "- [?] #task Mint me [dependsOn:: fix-apollo] [id:: mint-me]\n",
            ),
        )],
        Some(&day_with_fix_link()),
        "20261005.md",
        false,
        &vault_index,
    );
    assert_eq!(plan.unblocked.len(), 1);
    let row = &plan.unblocked[0];
    assert_eq!(row.not_linked, Some("disabled"));
    assert_eq!(row.block_id, "");
    assert!(plan.new_day_text.is_none());
    let updated = plan
        .changed_files
        .get(&PathBuf::from("sase.md"))
        .expect("recovery still writes");
    assert!(updated.contains("- [ ] #task Mint me"));
}

// SL23: a closed subtask with no live link of its own anchors at its
// root's link.
#[test]
fn sl23_subtask_inherits_root_anchor() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let plan = run(
        vec![
            pred("sase.md", "root", "root", "Root", true),
            pred("sase.md", "sub", "sub", "Sub", false),
        ],
        &[(
            "sase.md",
            concat!(
                "- [x] #task Root [id:: root] ^root\n",
                "- [?] #task Follows sub [dependsOn:: sub] [id:: follows] ^follows\n",
            ),
        )],
        Some(
            "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n- [ ] () \u{2014} FIX\n  - [[sase#^root]]\n",
        ),
        "20261005.md",
        true,
        &vault_index,
    );
    assert_eq!(plan.unblocked.len(), 1);
    assert!(plan.unblocked[0].link.is_some());
}

// An ID-less planned root with an ID-bearing embedded subtask: the
// subtask keeps the root entry for its anchor (no dependency ID is
// minted for the root), so the subtask's dependent links instead of
// recovering to `not_planned_today`.
#[test]
fn idless_root_anchors_id_bearing_child_dependent() {
    let vault_index = index(&["sase.md", "sub.md", "d.md", "20261005.md"]);
    let plan = run(
        vec![
            pred("sase.md", "p", "", "Root", true),
            pred("sub.md", "s", "s", "Subtask", false),
        ],
        &[
            (
                "sub.md",
                "- [x] #task Subtask [id:: s] ^s\n",
            ),
            (
                "d.md",
                "- [?] #task Dependent [dependsOn:: s] [id:: d] ^d\n",
            ),
        ],
        Some(
            "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n- [ ] () \u{2014} FIX\n  - [[sase#^p]]\n",
        ),
        "20261005.md",
        true,
        &vault_index,
    );
    assert_eq!(plan.unblocked.len(), 1);
    let row = &plan.unblocked[0];
    assert_eq!(row.block_id, "d");
    let link = row.link.as_ref().expect("linked at root anchor");
    assert_eq!(link.entry_name, "FIX");
    assert_eq!(link.block_link, "[[d#^d]]");
}

// SL25: an already-Next dependent that is already planned produces no
// row and no write.
#[test]
fn sl25_planned_next_reports_nothing() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
        "- [ ] () \u{2014} FIX\n",
        "  - [[sase#^fix-apollo]]\n",
        "  - [[sase#^relaunch]]\n",
    );
    let plan = run(
        vec![pred("sase.md", "fix-apollo", "fix-apollo", "Fix apollo", true)],
        &[(
            "sase.md",
            concat!(
                "- [x] #task Fix apollo [id:: fix-apollo] ^fix-apollo\n",
                "- [*] #task Relaunch [dependsOn:: fix-apollo] [id:: relaunch] ^relaunch\n",
            ),
        )],
        Some(day),
        "20261005.md",
        true,
        &vault_index,
    );
    assert!(plan.unblocked.is_empty());
    assert!(plan.still_blocked.is_empty());
    assert!(plan.changed_files.is_empty());
    assert!(plan.new_day_text.is_none());
}

// SL26: a successor living in an inbox file links with `inbox: true`.
// Inbox follows nav's `classifyInboxNote`: root `inbox.md`, plus direct
// area children whose frontmatter `parent` resolves to it.
#[test]
fn sl26_inbox_successor_links_with_flag() {
    let vault_index = index(&["sase.md", "inbox.md", "20261005.md"]);
    let plan = run(
        vec![pred("sase.md", "fix-apollo", "fix-apollo", "Fix apollo", true)],
        &[(
            "inbox.md",
            "- [?] #task Triage me [dependsOn:: fix-apollo] [id:: triage] ^triage\n",
        )],
        Some(&day_with_fix_link()),
        "20261005.md",
        true,
        &vault_index,
    );
    assert_eq!(plan.unblocked.len(), 1);
    assert!(plan.unblocked[0].inbox);
    assert!(plan.unblocked[0].link.is_some());
    let sase_contents = "- [?] #task Triage me [dependsOn:: fix-apollo] [id:: triage] ^triage\n";
    assert!(!super::super::successors::is_inbox_note(
        Path::new("sase.md"),
        sase_contents,
        &vault_index,
    ));
    assert!(super::super::successors::is_inbox_note(
        Path::new("inbox.md"),
        "- [?] #task Triage me [dependsOn:: fix-apollo] [id:: triage] ^triage\n",
        &vault_index,
    ));
}

// Inbox parity shared cases (mirrored in the cycler's inbox suite):
// root, renamed direct area child, mac/gkeep inboxes with and without
// the defining metadata, project child, untyped child, nested
// grandchild.
#[test]
fn inbox_parity_shared_cases() {
    use super::super::successors::is_inbox_note;
    let vault_index = index(&[
        "inbox.md",
        "areas/triage.md",
        "renamed.md",
        "mac_inbox.md",
        "gkeep_inbox.md",
        "proj.md",
        "plain.md",
        "areas/child.md",
        "areas/child/grand.md",
    ]);
    let area_parent = "---\ntype: [[area]]\nparent: [[inbox]]\n---\n";
    let task = "- [?] #task D [dependsOn:: p] [id:: d] ^d\n";
    // Root inbox counts with no metadata at all.
    assert!(is_inbox_note(Path::new("inbox.md"), task, &vault_index));
    // A renamed direct area child still counts: parent resolves by link.
    assert!(is_inbox_note(
        Path::new("renamed.md"),
        &format!("{area_parent}{task}"),
        &vault_index,
    ));
    // mac/gkeep capture inboxes count only with the defining metadata.
    assert!(is_inbox_note(
        Path::new("mac_inbox.md"),
        &format!("{area_parent}{task}"),
        &vault_index,
    ));
    assert!(!is_inbox_note(
        Path::new("mac_inbox.md"),
        task,
        &vault_index
    ));
    assert!(is_inbox_note(
        Path::new("gkeep_inbox.md"),
        &format!("{area_parent}{task}"),
        &vault_index,
    ));
    assert!(!is_inbox_note(
        Path::new("gkeep_inbox.md"),
        task,
        &vault_index
    ));
    // A project child never counts, whatever its parent.
    assert!(!is_inbox_note(
        Path::new("proj.md"),
        &format!("---\ntype: [[project]]\nparent: [[inbox]]\n---\n{task}"),
        &vault_index,
    ));
    // An untyped child never counts, whatever its parent.
    assert!(!is_inbox_note(
        Path::new("plain.md"),
        &format!("---\nparent: [[inbox]]\n---\n{task}"),
        &vault_index,
    ));
    // A nested grandchild's parent points at the area, not the inbox.
    assert!(!is_inbox_note(
        Path::new("areas/child/grand.md"),
        &format!("---\ntype: [[area]]\nparent: [[child]]\n---\n{task}"),
        &vault_index,
    ));
    // Without a vault inbox, even an area child pointing at it is out.
    let no_inbox = index(&["areas/triage.md"]);
    assert!(!is_inbox_note(
        Path::new("areas/triage.md"),
        &format!("{area_parent}{task}"),
        &no_inbox,
    ));
}

// SL28: a successor living in the day file itself names the note in its
// link, never the bare `[[#^id]]`.
#[test]
fn sl28_day_file_successor_names_note() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
        "- [ ] () \u{2014} FIX\n",
        "  - [[sase#^fix-apollo]]\n",
        "## Tasks\n",
        "- [?] #task Day job [dependsOn:: fix-apollo] [id:: day-job] ^day-job\n",
    );
    let plan = run(
        vec![pred("sase.md", "fix-apollo", "fix-apollo", "Fix apollo", true)],
        &[(
            "20261005.md",
            "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n- [ ] () \u{2014} FIX\n  - [[sase#^fix-apollo]]\n## Tasks\n- [?] #task Day job [dependsOn:: fix-apollo] [id:: day-job] ^day-job\n",
        )],
        Some(day),
        "20261005.md",
        true,
        &vault_index,
    );
    assert_eq!(plan.unblocked.len(), 1);
    let link = plan.unblocked[0].link.as_ref().expect("linked");
    assert_eq!(link.block_link, "[[20261005#^day-job]]");
}

// Successors under one anchor keep successor order; the target entry
// reports its post line and next-up state.
#[test]
fn slot_placement_orders_and_reports_lines() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let day = day_with_fix_link();
    let (text, placed) = insert_successor_links(
        &day,
        Path::new("20261005.md"),
        &vault_index,
        vec![
            SuccessorPlacement {
                target: SuccessorTarget::Slot {
                    entry_index: 1,
                    bullet_end: 5,
                    indent: "  ".to_string(),
                },
                block_link: "[[sase#^aaa]]".to_string(),
            },
            SuccessorPlacement {
                target: SuccessorTarget::Slot {
                    entry_index: 1,
                    bullet_end: 5,
                    indent: "  ".to_string(),
                },
                block_link: "[[sase#^mmm]]".to_string(),
            },
        ],
    );
    assert_eq!(placed.len(), 2);
    assert!(placed[0].line < placed[1].line);
    assert_eq!(placed[0].entry_line, 4);
    assert!(!placed[0].entry_created);
    assert!(text.contains("  - [[sase#^aaa]]\n  - [[sase#^mmm]]\n"));
}

// Closing targets (wired by `capture_close`): a created placeholder
// goes right after the closed entry holding only the successors.
#[test]
fn closing_placement_creates_placeholder() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let day = concat!(
        "## Pomodoros\n",
        "- [x] (**0900-0920**) \u{2014} BOB\n",
        "  - ~~[[sase#^done]]~~\n",
        "- [ ] () \u{2014} FIX\n",
    );
    let (text, placed) = insert_successor_links(
        &day,
        Path::new("20261005.md"),
        &vault_index,
        vec![SuccessorPlacement {
            target: SuccessorTarget::Closing {
                entry_index: None,
                insert_at: 3,
                created_name: Some("BOB".to_string()),
            },
            block_link: "[[sase#^next]]".to_string(),
        }],
    );
    assert_eq!(placed.len(), 1);
    assert!(placed[0].entry_created);
    assert!(text.contains("- [ ] () \u{2014} BOB\n\t- [[sase#^next]]\n"));
}

// A single closing placement into an existing entry whose only child is
// a lone stub takes the stub line in place: no panic, no leftover blank
// bullet, and the reported line points at the link.
#[test]
fn closing_placement_takes_lone_stub() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let day = concat!(
        "## Pomodoros\n",
        "- [x] (**0900-0920**) \u{2014} BOB\n",
        "  - ~~[[sase#^done]]~~\n",
        "- [ ] () \u{2014} BOB\n",
        "\t- \n",
        "- [ ] () \u{2014} FIX\n",
    );
    let (text, placed) = insert_successor_links(
        &day,
        Path::new("20261005.md"),
        &vault_index,
        vec![SuccessorPlacement {
            target: SuccessorTarget::Closing {
                entry_index: Some(1),
                insert_at: 5,
                created_name: None,
            },
            block_link: "[[sase#^next]]".to_string(),
        }],
    );
    assert_eq!(placed.len(), 1);
    assert!(!placed[0].entry_created);
    assert_eq!(placed[0].entry_line, 4);
    assert_eq!(placed[0].line, 5);
    assert!(text.contains("- [ ] () \u{2014} BOB\n\t- [[sase#^next]]\n"));
    assert!(!text.contains("\t- \n"));
}

fn project_note(body: &str) -> String {
    format!("---\ntype: [[project]]\nstatus: wip\n---\n{body}")
}

// PDC1 (§13.16): D lives in a still-blocked project; this close
// removed D's last own prerequisite. D stays `[?]`;
// `still_blocked{reason: project}` with the project path; no
// successor link and no note edit.
#[test]
fn pdc1_dependent_in_still_blocked_project_stays_blocked() {
    let vault_index = index(&["sase.md", "job.md", "proj.md", "20261005.md"]);
    let plan = run(
        vec![pred("sase.md", "done-p", "done-p", "Done P", true)],
        &[
            (
                "sase.md",
                "- [x] #task Done P [id:: done-p] ^done-p\n",
            ),
            (
                "job.md",
                "- [ ] #task LinkedIn [id:: job__linkedin] ^linkedin\n",
            ),
            (
                "proj.md",
                &project_note(concat!(
                    "- [?] #task Apply [dependsOn:: job__linkedin] [id:: apply] ^prj\n",
                    "- [?] #task Resident [dependsOn:: done-p] [id:: res] ^res\n",
                )),
            ),
        ],
        Some(&day_with_fix_link()),
        "20261005.md",
        true,
        &vault_index,
    );
    assert!(plan.unblocked.is_empty(), "{:?}", plan.unblocked);
    assert_eq!(plan.still_blocked.len(), 1);
    let row = &plan.still_blocked[0];
    assert_eq!(row.block_id, "res");
    assert_eq!(row.reason, "project");
    assert_eq!(row.status_symbol, '?');
    assert_eq!(row.project, Some(PathBuf::from("proj.md")));
    assert!(plan.new_day_text.is_none());
    assert!(plan.changed_files.is_empty());
}

// PDC2: closing P fully unblocks `^prj`; the six residents are
// `project` offers. All links them at `earliest_anchor(preds(R))`
// (FIX / `done-p`), stamps `[fresh:: 2026-10-05]`, and does not write
// `[unblocked::]` on those picked lines. `^prj` stays `project_task`,
// recovers to Ready, and keeps the `[unblocked::]` WRITE. The outside
// dependent stays one `hand_off` row. There is no breaker.
#[test]
fn pdc2_prj_recovery_offers_residents() {
    let vault_index = index(&["sase.md", "proj.md", "20261005.md"]);
    let mut body =
        "- [?] #task Apply [fresh:: 2026-10-01] [dependsOn:: done-p] [id:: apply] ^prj\n"
            .to_string();
    for n in 1..=6 {
        body.push_str(&format!(
            "- [?] #task Pitch {n} [fresh:: 2026-10-01] #prj_is_blocked [id:: pitch-{n}] ^pitch-{n}\n"
        ));
    }
    let day =
        day_with_fix_link().replace("[[sase#^fix-apollo]]", "[[sase#^done-p]]");
    let plan = run(
        vec![pred("sase.md", "done-p", "done-p", "Done P", true)],
        &[
            (
                "sase.md",
                concat!(
                    "- [x] #task Done P [id:: done-p] ^done-p\n",
                    "- [?] #task Outside [fresh:: 2026-10-01] [dependsOn:: done-p] [id:: outside] ^outside\n",
                ),
            ),
            ("proj.md", &project_note(&body)),
        ],
        Some(&day),
        "20261005.md",
        true,
        &vault_index,
    );
    let members: Vec<_> = plan
        .unblocked
        .iter()
        .filter(|row| {
            row.offer
                .as_ref()
                .is_some_and(|offer| offer.group == OfferGroup::Project)
        })
        .collect();
    assert_eq!(members.len(), 6, "{:?}", plan.unblocked);
    let mut project_ns = Vec::new();
    for row in &members {
        assert_eq!(row.previous_status_symbol, '?');
        assert_eq!(row.status_symbol, '*');
        assert!(row.link.is_some());
        assert_eq!(row.not_linked, None);
        let offer = row.offer.as_ref().expect("project offer");
        assert!(offer.selected);
        assert_eq!(offer.destination.entry_name, "FIX");
        assert!(!offer.destination.fallback);
        assert_eq!(offer.project, Some(PathBuf::from("proj.md")));
        project_ns.push(offer.n);
    }
    project_ns.sort_unstable();
    assert_eq!(project_ns, vec![2, 3, 4, 5, 6, 7]);
    let prj = plan
        .unblocked
        .iter()
        .find(|row| row.block_id == "prj")
        .expect("prj recovered");
    assert_eq!(prj.not_linked, Some("project_task"));
    assert!(prj.offer.is_none());
    assert!(prj.link.is_none());
    let outside = plan
        .unblocked
        .iter()
        .find(|row| row.block_id == "outside")
        .expect("outside successor");
    assert!(outside.link.is_some());
    assert_eq!(outside.not_linked, None);
    let outside_offer = outside.offer.as_ref().expect("hand_off");
    assert_eq!(outside_offer.group, OfferGroup::HandOff);
    assert_eq!(outside_offer.n, 1);
    assert!(plan.still_blocked.is_empty());
    let updated = plan
        .changed_files
        .get(&PathBuf::from("proj.md"))
        .expect("proj.md changed");
    assert!(!updated.contains("#prj_is_blocked"), "{updated}");
    assert!(updated.contains("- [ ] #task Apply"), "{updated}");
    assert!(
        updated.contains("- [*] #task Pitch 1 [fresh:: 2026-10-05]"),
        "{updated}"
    );
    let apply_line = updated
        .lines()
        .find(|line| line.contains("^prj"))
        .expect("apply line");
    assert!(
        apply_line.contains("[unblocked:: 2026-10-05]"),
        "{apply_line}"
    );
    let pitch_line = updated
        .lines()
        .find(|line| line.contains("^pitch-1"))
        .expect("pitch line");
    assert!(!pitch_line.contains("[unblocked::"), "{pitch_line}");
}

// PDC3 / SO8 / SO9: the sequenced resident is a muted `still_blocked`
// row (`waits_on`, `project: proj.md`) and loses `#prj_is_blocked`.
// The `[ ]` ready leftover is offered even though its status is
// unchanged until picked (SO8). This fixture's day links `fix-apollo`,
// not `done-p`, so the resident destination is implicit (CAPTURE).
#[test]
fn pdc3_resident_with_own_prerequisite_stays_blocked() {
    let vault_index = index(&["sase.md", "proj.md", "20261005.md"]);
    let plan = run(
        vec![pred("sase.md", "done-p", "done-p", "Done P", true)],
        &[
            (
                "sase.md",
                "- [x] #task Done P [id:: done-p] ^done-p\n- [ ] #task Extra [id:: extra] ^extra\n",
            ),
            (
                "proj.md",
                &project_note(concat!(
                    "- [?] #task Apply [fresh:: 2026-10-01] [dependsOn:: done-p] [id:: apply] ^prj\n",
                    "- [?] #task Pitch [fresh:: 2026-10-01] [id:: pitch] ^pitch\n",
                    "- [?] #task Sequenced [fresh:: 2026-10-01] [dependsOn:: extra] #prj_is_blocked [id:: seq] ^seq\n",
                    "- [ ] #task Ready leftover #prj_is_blocked [id:: ready] ^ready\n",
                )),
            ),
        ],
        Some(&day_with_fix_link()),
        "20261005.md",
        true,
        &vault_index,
    );
    let ids: Vec<&str> = plan
        .unblocked
        .iter()
        .map(|row| row.block_id.as_str())
        .collect();
    assert!(ids.contains(&"prj"), "{ids:?}");
    assert!(ids.contains(&"pitch"), "{ids:?}");
    assert!(ids.contains(&"ready"), "{ids:?}");
    assert!(!ids.contains(&"seq"), "{ids:?}");
    let pitch = plan
        .unblocked
        .iter()
        .find(|row| row.block_id == "pitch")
        .expect("pitch");
    let pitch_offer = pitch.offer.as_ref().expect("project offer");
    assert_eq!(pitch_offer.group, OfferGroup::Project);
    assert!(pitch_offer.selected);
    assert_eq!(pitch_offer.destination.entry_name, "CAPTURE");
    assert!(pitch_offer.destination.fallback);
    assert_eq!(pitch_offer.destination.entry_role, EntryRole::Implicit);
    assert!(pitch.link.is_some());
    assert_eq!(pitch.not_linked, None);
    let ready = plan
        .unblocked
        .iter()
        .find(|row| row.block_id == "ready")
        .expect("ready leftover offered (SO8)");
    assert_eq!(ready.previous_status_symbol, ' ');
    assert_eq!(ready.status_symbol, '*');
    assert!(ready.offer.is_some());
    assert_eq!(plan.still_blocked.len(), 1);
    let seq = &plan.still_blocked[0];
    assert_eq!(seq.block_id, "seq");
    assert_eq!(seq.reason, "waits_on");
    assert_eq!(seq.project, Some(PathBuf::from("proj.md")));
    let updated = plan
        .changed_files
        .get(&PathBuf::from("proj.md"))
        .expect("proj.md changed");
    assert!(updated.contains("- [?] #task Sequenced"), "{updated}");
    assert!(
        updated.contains("- [*] #task Pitch [fresh:: 2026-10-05]"),
        "{updated}"
    );
    assert!(!updated.contains("#prj_is_blocked"), "{updated}");
    assert!(
        updated.contains("- [*] #task Ready leftover [fresh:: 2026-10-05]"),
        "{updated}"
    );
}

// direct_prj_link = raise: a `^prj` that keeps a live Task Link
// recovers to Next, matching reconcile.
#[test]
fn linked_prj_recovery_lands_next() {
    let vault_index = index(&["sase.md", "proj.md", "20261005.md"]);
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
        "  - [[sase#^other]]\n",
        "- [ ] () \u{2014} FIX\n",
        "  - [[proj#^prj]]\n",
    );
    let plan = run(
        vec![pred("sase.md", "done-p", "done-p", "Done P", true)],
        &[
            (
                "sase.md",
                "- [x] #task Done P [id:: done-p] ^done-p\n",
            ),
            (
                "proj.md",
                &project_note(
                    "- [?] #task Apply [dependsOn:: done-p] [id:: apply] ^prj\n",
                ),
            ),
        ],
        Some(day),
        "20261005.md",
        true,
        &vault_index,
    );
    let prj = plan
        .unblocked
        .iter()
        .find(|row| row.block_id == "prj")
        .expect("prj recovered");
    // §12.10 pull-in (PP9 shape via trigger (b): N holds only R, so the
    // pull-in replaces the SL11 `project_task` row; step 5 keeps the live
    // link with no new link and still lands Next).
    assert_eq!(prj.not_linked, Some("already_planned"));
    assert_eq!(prj.trigger, "project_empty");
    assert_eq!(prj.status_symbol, '*');
    let updated = plan
        .changed_files
        .get(&PathBuf::from("proj.md"))
        .expect("proj.md changed");
    assert!(updated.contains("- [*] #task Apply"), "{updated}");
}

// UB1 at close level (`docs/freshness.md` §10b): a Ready recovery
// with `[fresh::]` writes `[unblocked:: today]`.
#[test]
fn ub_close_ready_recovery_writes_unblocked() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let plan = run_sel(
        vec![pred("sase.md", "p", "p", "P", true)],
        &[(
            "sase.md",
            concat!(
                "- [x] #task P [id:: p] ^p\n",
                "- [?] #task D [fresh:: 2026-10-01] [dependsOn:: p] [id:: d] ^d\n",
            ),
        )],
        Some(&day_with_fix_link()),
        "20261005.md",
        true,
        &vault_index,
        SuccessorSelection::None,
    );
    // Unpicked implicit offer: D recovers `not_selected` to Ready and
    // the WRITE rule still stamps `[unblocked::]`.
    let row = plan
        .unblocked
        .iter()
        .find(|row| row.block_id == "d")
        .expect("d recovered");
    assert_eq!(row.status_symbol, ' ');
    assert_eq!(row.not_linked, Some("not_selected"));
    let updated = plan
        .changed_files
        .get(&PathBuf::from("sase.md"))
        .expect("sase.md changed");
    assert!(updated.contains("[unblocked:: 2026-10-05]"), "{updated}");
}

// UB2 at close level: a recovered NEW row (no `[fresh::]`) writes no
// `[unblocked::]`.
#[test]
fn ub_close_new_recovery_skips_unblocked() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let plan = run_sel(
        vec![pred("sase.md", "p", "p", "P", true)],
        &[(
            "sase.md",
            concat!(
                "- [x] #task P [id:: p] ^p\n",
                "- [?] #task D [dependsOn:: p] [id:: d] ^d\n",
            ),
        )],
        Some(&day_with_fix_link()),
        "20261005.md",
        true,
        &vault_index,
        SuccessorSelection::None,
    );
    let updated = plan
        .changed_files
        .get(&PathBuf::from("sase.md"))
        .expect("sase.md changed");
    assert!(updated.contains("- [ ] #task D"), "{updated}");
    assert!(!updated.contains("[unblocked::"), "{updated}");
}

// UB3 at close level: a recovered scheduled tickler
// (`fresh < scheduled ≤ today`) writes no `[unblocked::]`.
#[test]
fn ub_close_scheduled_tickler_skips_unblocked() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let plan = run_sel(
        vec![pred("sase.md", "p", "p", "P", true)],
        &[(
            "sase.md",
            concat!(
                "- [x] #task P [id:: p] ^p\n",
                "- [?] #task D [fresh:: 2026-10-01] [dependsOn:: p] [scheduled:: 2026-10-04] [id:: d] ^d\n",
            ),
        )],
        Some(&day_with_fix_link()),
        "20261005.md",
        true,
        &vault_index,
        SuccessorSelection::None,
    );
    let updated = plan
        .changed_files
        .get(&PathBuf::from("sase.md"))
        .expect("sase.md changed");
    assert!(updated.contains("- [ ] #task D"), "{updated}");
    assert!(!updated.contains("[unblocked::"), "{updated}");
}

// UB4 at close level: a recovery that lands in Next (a kept live
// link) writes no `[unblocked::]`; the lane walk covers it.
#[test]
fn ub_close_next_landing_skips_unblocked() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
        "  - [[sase#^p]]\n",
        "- [ ] () \u{2014} FIX\n",
        "  - [[sase#^d]]\n",
    );
    let plan = run(
        vec![pred("sase.md", "p", "p", "P", true)],
        &[(
            "sase.md",
            concat!(
                "- [x] #task P [id:: p] ^p\n",
                "- [?] #task D [fresh:: 2026-10-01] [dependsOn:: p] [id:: d] ^d\n",
            ),
        )],
        Some(&day),
        "20261005.md",
        true,
        &vault_index,
    );
    let row = plan
        .unblocked
        .iter()
        .find(|row| row.block_id == "d")
        .expect("d recovered");
    assert_eq!(row.not_linked, Some("already_planned"));
    assert_eq!(row.status_symbol, '*');
    let updated = plan
        .changed_files
        .get(&PathBuf::from("sase.md"))
        .expect("sase.md changed");
    assert!(!updated.contains("[unblocked::"), "{updated}");
}

// UB6 at close level: a hidden resident still recovers with its
// `^prj`, but the hidden scope skips `[unblocked::]`.
#[test]
fn ub_close_hidden_resident_recovers_without_unblocked() {
    let vault_index = index(&["sase.md", "proj.md", "20261005.md"]);
    let plan = run(
        vec![pred("sase.md", "done-p", "done-p", "Done P", true)],
        &[
            (
                "sase.md",
                "- [x] #task Done P [id:: done-p] ^done-p\n",
            ),
            (
                "proj.md",
                &project_note(concat!(
                    "- [?] #task Apply [fresh:: 2026-10-01] [dependsOn:: done-p] [id:: apply] ^prj\n",
                    "- [?] #task Quiet [fresh:: 2026-10-01] #hide [id:: quiet] ^quiet\n",
                )),
            ),
        ],
        Some(&day_with_fix_link()),
        "20261005.md",
        true,
        &vault_index,
    );
    let quiet = plan
        .unblocked
        .iter()
        .find(|row| row.block_id == "quiet")
        .expect("quiet recovered");
    assert_eq!(quiet.not_linked, Some("hidden"));
    assert_eq!(quiet.status_symbol, ' ');
    let updated = plan
        .changed_files
        .get(&PathBuf::from("proj.md"))
        .expect("proj.md changed");
    let quiet_line = updated
        .lines()
        .find(|line| line.contains("^quiet"))
        .expect("quiet line");
    assert!(quiet_line.contains("- [ ] #task Quiet"), "{quiet_line}");
    assert!(!quiet_line.contains("[unblocked::"), "{quiet_line}");
}

// Project note with a scalar `parent:` frontmatter link, read the way
// sync reads it (`docs/task-dependencies.md` §13.1).
fn child_project_note(parent: &str, body: &str) -> String {
    format!(
        "---\ntype: [[project]]\nstatus: wip\nparent: \"[[{parent}]]\"\n---\n{body}"
    )
}

// PDC4 (§13.16): D is a resident of K whose `^prj` carries
// `#prj_is_blocked`; this close removes D's last own prerequisite, but D
// stays `[?]` with `still_blocked{reason: project, project: parent(K)}`
// and no link.
#[test]
fn pdc4_inherited_resident_stays_blocked_naming_parent() {
    let vault_index = index(&["sase.md", "k.md", "par.md", "20261005.md"]);
    let plan = run(
        vec![pred("sase.md", "done-p", "done-p", "Done P", true)],
        &[
            (
                "sase.md",
                "- [x] #task Done P [id:: done-p] ^done-p\n",
            ),
            (
                "k.md",
                &child_project_note(
                    "par",
                    concat!(
                        "- [?] #task Kid [fresh:: 2026-10-01] #prj_is_blocked ^prj\n",
                        "- [?] #task Dee [fresh:: 2026-10-01] [dependsOn:: done-p] #prj_is_blocked [id:: dee] ^dee\n",
                    ),
                ),
            ),
        ],
        Some(&day_with_fix_link()),
        "20261005.md",
        true,
        &vault_index,
    );
    assert!(plan.unblocked.is_empty(), "{:?}", plan.unblocked);
    assert_eq!(plan.still_blocked.len(), 1);
    let row = &plan.still_blocked[0];
    assert_eq!(row.block_id, "dee");
    assert_eq!(row.reason, "project");
    assert_eq!(row.project, Some(PathBuf::from("par.md")));
    assert_eq!(row.trigger, "dependency");
    assert!(
        !plan.changed_files.contains_key(&PathBuf::from("k.md")),
        "D stays byte-identical: {:?}",
        plan.changed_files
    );
}

// PDC5 (§13.16): K's tagged `^prj` loses its last own prerequisite →
// stays `[?]` with `still_blocked{reason: project}`; no `project_task`
// recover and no PDC2/PDC3 for K's residents (they keep their tags
// until reconcile).
#[test]
fn pdc5_tagged_prj_loses_last_prerequisite_stays_blocked() {
    let vault_index = index(&["sase.md", "k.md", "par.md", "20261005.md"]);
    let plan = run(
        vec![pred("sase.md", "done-p", "done-p", "Done P", true)],
        &[
            (
                "sase.md",
                "- [x] #task Done P [id:: done-p] ^done-p\n",
            ),
            (
                "k.md",
                &child_project_note(
                    "par",
                    concat!(
                        "- [?] #task Kid [fresh:: 2026-10-01] [dependsOn:: done-p] #prj_is_blocked [id:: kprj] ^prj\n",
                        "- [?] #task Rita [fresh:: 2026-10-01] #prj_is_blocked [id:: rita] ^rita\n",
                    ),
                ),
            ),
        ],
        Some(&day_with_fix_link()),
        "20261005.md",
        true,
        &vault_index,
    );
    assert!(plan.unblocked.is_empty(), "{:?}", plan.unblocked);
    assert_eq!(plan.still_blocked.len(), 1);
    let row = &plan.still_blocked[0];
    assert_eq!(row.block_id, "prj");
    assert_eq!(row.reason, "project");
    assert_eq!(row.project, Some(PathBuf::from("par.md")));
    assert_eq!(row.trigger, "dependency");
    assert!(
        !plan.changed_files.contains_key(&PathBuf::from("k.md")),
        "K stays byte-identical, residents keep their tags: {:?}",
        plan.changed_files
    );
}

// PDC6 under `close_recovers_subprojects = no` (§13.16): a close that
// fully unblocks an untagged N recovers only N's own residents (PDC2);
// a tagged descendant K in the snapshot keeps its tags until the next
// reconcile.
#[test]
fn pdc6_no_close_recovery_for_tagged_descendants() {
    let vault_index =
        index(&["sase.md", "proj.md", "k.md", "par.md", "20261005.md"]);
    let plan = run(
        vec![pred("sase.md", "done-p", "done-p", "Done P", true)],
        &[
            (
                "sase.md",
                "- [x] #task Done P [id:: done-p] ^done-p\n",
            ),
            (
                "proj.md",
                &project_note(concat!(
                    "- [?] #task Apply [fresh:: 2026-10-01] [dependsOn:: done-p] [id:: apply] ^prj\n",
                    "- [?] #task Pitch [fresh:: 2026-10-01] #prj_is_blocked [id:: pitch] ^pitch\n",
                )),
            ),
            (
                "k.md",
                &child_project_note(
                    "proj",
                    concat!(
                        "- [?] #task Kid [fresh:: 2026-10-01] #prj_is_blocked ^prj\n",
                        "- [?] #task Rita [fresh:: 2026-10-01] #prj_is_blocked [id:: rita] ^rita\n",
                    ),
                ),
            ),
        ],
        Some(&day_with_fix_link()),
        "20261005.md",
        true,
        &vault_index,
    );
    let pitch = plan
        .unblocked
        .iter()
        .find(|row| row.block_id == "pitch")
        .expect("N's own resident is offered");
    let pitch_offer = pitch.offer.as_ref().expect("project offer");
    assert_eq!(pitch_offer.group, OfferGroup::Project);
    assert!(pitch_offer.selected);
    assert_eq!(pitch.not_linked, None);
    assert!(pitch.link.is_some());
    assert!(
        !plan.changed_files.contains_key(&PathBuf::from("k.md")),
        "tagged descendant keeps its tags until reconcile: {:?}",
        plan.changed_files
    );
    assert!(plan.still_blocked.is_empty(), "{:?}", plan.still_blocked);
}

fn so1_snapshot() -> (String, String) {
    (
        "- [x] #task Bar [id:: bar] ^bar\n".to_string(),
        project_note(concat!(
            "- [?] #task Foo [dependsOn:: bar] [id:: foo] ^prj\n",
            "- [?] #task First [unblocked:: 2026-10-01] [id:: first] ^first\n",
            "- [?] #task Second\n",
        )),
    )
}

fn so1_day() -> String {
    concat!(
        "## Pomodoros\n",
        "- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
        "- [ ] () \u{2014} FIX\n",
        "  - [[sase#^bar]]\n",
    )
    .to_string()
}

// SO1: close `sase.md` `^bar`; `sase_foo.md` `^prj` depended only on
// `^bar` and has two open residents. Both are `project` offers, `^prj`
// is not an offer. Picking the id-less row mints, sets `[*]`, stamps
// `[fresh:: today]`, clears `[unblocked::]`, and links under `^bar`'s
// entry. The other recovers `not_selected` with no mint.
#[test]
fn so1_project_offers_pick_one() {
    let vault_index = index(&["sase.md", "sase_foo.md", "20261005.md"]);
    let (sase, foo) = so1_snapshot();
    let day = so1_day();
    let preview = run_sel(
        vec![pred("sase.md", "bar", "bar", "Bar", true)],
        &[("sase.md", &sase), ("sase_foo.md", &foo)],
        Some(&day),
        "20261005.md",
        true,
        &vault_index,
        SuccessorSelection::None,
    );
    let pick_key = preview
        .unblocked
        .iter()
        .find(|row| row.block_id.is_empty())
        .and_then(|row| row.offer.as_ref().map(|offer| offer.key.clone()))
        .expect("id-less resident key");
    let plan = run_sel(
        vec![pred("sase.md", "bar", "bar", "Bar", true)],
        &[("sase.md", &sase), ("sase_foo.md", &foo)],
        Some(&day),
        "20261005.md",
        true,
        &vault_index,
        SuccessorSelection::Keys(BTreeSet::from([pick_key])),
    );
    let prj = plan
        .unblocked
        .iter()
        .find(|row| row.block_id == "prj")
        .expect("^prj is the group header");
    assert_eq!(prj.not_linked, Some("project_task"));
    assert!(prj.offer.is_none());
    let project_offers: Vec<_> = plan
        .unblocked
        .iter()
        .filter(|row| {
            row.offer
                .as_ref()
                .is_some_and(|offer| offer.group == OfferGroup::Project)
        })
        .collect();
    assert_eq!(project_offers.len(), 2, "{:?}", plan.unblocked);
    let picked = project_offers
        .iter()
        .find(|row| row.offer.as_ref().is_some_and(|offer| offer.selected))
        .expect("picked resident");
    assert!(picked.block_id_is_minted(), "{picked:?}");
    assert_eq!(picked.status_symbol, '*');
    assert!(picked.link.is_some());
    assert_eq!(picked.link.as_ref().unwrap().entry_name, "FIX");
    assert!(picked.link.as_ref().unwrap().block_id_created);
    let unpicked = project_offers
        .iter()
        .find(|row| row.offer.as_ref().is_some_and(|offer| !offer.selected))
        .expect("unpicked resident");
    assert_eq!(unpicked.not_linked, Some("not_selected"));
    assert!(unpicked.link.is_none());
    assert_eq!(unpicked.block_id, "first");
    let updated = plan
        .changed_files
        .get(&PathBuf::from("sase_foo.md"))
        .expect("sase_foo.md changed");
    let second_line = updated
        .lines()
        .find(|line| line.contains("Second"))
        .expect("second line");
    assert!(second_line.contains("- [*] #task Second"), "{second_line}");
    assert!(
        second_line.contains("[fresh:: 2026-10-05]"),
        "{second_line}"
    );
    assert!(second_line.contains(" ^"), "{second_line}");
    assert!(!second_line.contains("[unblocked::"), "{second_line}");
    let first_line = updated
        .lines()
        .find(|line| line.contains("^first"))
        .expect("first line");
    assert!(first_line.contains("- [ ] #task First"), "{first_line}");
    assert!(!first_line.contains("[fresh:: 2026-10-05]"), "{first_line}");
}

trait BlockIdMinted {
    fn block_id_is_minted(&self) -> bool;
}

impl BlockIdMinted for super::super::successors::UnblockedRow {
    fn block_id_is_minted(&self) -> bool {
        self.link.as_ref().is_some_and(|link| link.block_id_created)
            && !self.block_id.is_empty()
    }
}

fn three_fan_note() -> String {
    concat!(
        "- [x] #task Fix apollo [id:: fix-apollo] ^fix-apollo\n",
        "- [?] #task Fan 1 [dependsOn:: fix-apollo] [id:: fan-1] ^fan-1\n",
        "- [?] #task Fan 2 [dependsOn:: fix-apollo] [id:: fan-2] ^fan-2\n",
        "- [?] #task Fan 3 [dependsOn:: fix-apollo] [id:: fan-3] ^fan-3\n",
    )
    .to_string()
}

// SO3: pick all — every selectable offer linked, raised to Next, stamped.
#[test]
fn so3_pick_all_links_every_offer() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let note = three_fan_note();
    let plan = run(
        vec![pred(
            "sase.md",
            "fix-apollo",
            "fix-apollo",
            "Fix apollo",
            true,
        )],
        &[("sase.md", &note)],
        Some(&day_with_fix_link()),
        "20261005.md",
        true,
        &vault_index,
    );
    assert_eq!(plan.unblocked.len(), 3);
    for row in &plan.unblocked {
        assert!(row.link.is_some());
        assert_eq!(row.status_symbol, '*');
        assert!(row.offer.as_ref().is_some_and(|offer| offer.selected));
    }
    let updated = plan
        .changed_files
        .get(&PathBuf::from("sase.md"))
        .expect("sase.md changed");
    assert!(updated.contains("[fresh:: 2026-10-05]"), "{updated}");
}

// SO4: pick none — recovery stands; no minted IDs; each offer
// `not_selected`.
#[test]
fn so4_pick_none_recovers_without_links() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let note = concat!(
        "- [x] #task Fix apollo [id:: fix-apollo] ^fix-apollo\n",
        "- [?] #task Fan 1 [dependsOn:: fix-apollo] [id:: fan-1] ^fan-1\n",
        "- [?] #task Mint me [dependsOn:: fix-apollo] [id:: mint-me]\n",
    );
    let plan = run_sel(
        vec![pred(
            "sase.md",
            "fix-apollo",
            "fix-apollo",
            "Fix apollo",
            true,
        )],
        &[("sase.md", note)],
        Some(&day_with_fix_link()),
        "20261005.md",
        true,
        &vault_index,
        SuccessorSelection::None,
    );
    assert_eq!(plan.unblocked.len(), 2);
    for row in &plan.unblocked {
        assert_eq!(row.not_linked, Some("not_selected"));
        assert!(row.link.is_none());
        assert!(!row.offer.as_ref().unwrap().selected);
    }
    assert!(plan.new_day_text.is_none());
    let updated = plan
        .changed_files
        .get(&PathBuf::from("sase.md"))
        .expect("recovery still writes");
    assert!(updated.contains("- [ ] #task Mint me"));
    assert!(!updated.contains(" ^mint-me"), "{updated}");
}

// SO5: pick a subset — only those rows linked; the rest `not_selected`.
#[test]
fn so5_pick_subset() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let note = three_fan_note();
    let plan = run_sel(
        vec![pred(
            "sase.md",
            "fix-apollo",
            "fix-apollo",
            "Fix apollo",
            true,
        )],
        &[("sase.md", &note)],
        Some(&day_with_fix_link()),
        "20261005.md",
        true,
        &vault_index,
        SuccessorSelection::Keys(BTreeSet::from(
            ["sase.md#^fan-2".to_string()],
        )),
    );
    let linked: Vec<_> = plan
        .unblocked
        .iter()
        .filter(|row| row.link.is_some())
        .collect();
    assert_eq!(linked.len(), 1);
    assert_eq!(linked[0].block_id, "fan-2");
    for row in &plan.unblocked {
        if row.block_id == "fan-2" {
            assert_eq!(row.not_linked, None);
            assert!(row.offer.as_ref().unwrap().selected);
        } else {
            assert_eq!(row.not_linked, Some("not_selected"));
            assert!(row.link.is_none());
        }
    }
}

// SO7: a resident that is also a direct dependent is one `hand_off` row.
#[test]
fn so7_direct_dependent_resident_stays_hand_off() {
    let vault_index = index(&["sase.md", "proj.md", "20261005.md"]);
    let plan = run(
        vec![pred("sase.md", "p", "p", "P", true)],
        &[
            ("sase.md", "- [x] #task P [id:: p] ^p\n"),
            (
                "proj.md",
                &project_note(concat!(
                    "- [?] #task Apply [dependsOn:: p] [id:: apply] ^prj\n",
                    "- [?] #task Also dep [dependsOn:: p] [id:: also] ^also\n",
                    "- [ ] #task Leftover [id:: left] ^left\n",
                )),
            ),
        ],
        Some("## Pomodoros\n- [ ] () \u{2014} FIX\n  - [[sase#^p]]\n"),
        "20261005.md",
        true,
        &vault_index,
    );
    let also = plan
        .unblocked
        .iter()
        .find(|row| row.block_id == "also")
        .expect("direct dependent");
    let also_offer = also.offer.as_ref().expect("one row");
    assert_eq!(also_offer.group, OfferGroup::HandOff);
    assert!(also.link.is_some());
    let leftover = plan
        .unblocked
        .iter()
        .find(|row| row.block_id == "left")
        .expect("plain resident");
    assert_eq!(
        leftover.offer.as_ref().map(|offer| offer.group),
        Some(OfferGroup::Project)
    );
}

// SO10, SO11, SO24: hidden and already-planned counts, and
// `open_subprojects` from the marker, with descendants unedited.
#[test]
fn so10_so11_so24_folds_and_open_subprojects() {
    let vault_index = index(&["sase.md", "proj.md", "k.md", "20261005.md"]);
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
        "- [ ] () \u{2014} FIX\n",
        "  - [[sase#^p]]\n",
        "  - [[proj#^planned]]\n",
    );
    let plan = run_sel(
        vec![pred("sase.md", "p", "p", "P", true)],
        &[
            ("sase.md", "- [x] #task P [id:: p] ^p\n"),
            (
                "proj.md",
                &project_note(concat!(
                    "- [?] #task Apply [dependsOn:: p] [id:: apply] ^prj\n",
                    "  - 🧩 **Sub-projects:** [[k]]\n",
                    "- [?] #task Quiet #hide [id:: quiet] ^quiet\n",
                    "- [?] #task Planned [id:: planned] ^planned\n",
                    "- [?] #task Offer me [id:: offer] ^offer\n",
                )),
            ),
            (
                "k.md",
                &child_project_note(
                    "proj",
                    concat!(
                        "- [?] #task Kid #prj_is_blocked ^prj\n",
                        "- [?] #task Rita #prj_is_blocked [id:: rita] ^rita\n",
                    ),
                ),
            ),
        ],
        Some(day),
        "20261005.md",
        true,
        &vault_index,
        SuccessorSelection::None,
    );
    assert_eq!(plan.project_folds.len(), 1);
    let fold = &plan.project_folds[0];
    assert_eq!(fold.project, PathBuf::from("proj.md"));
    assert_eq!(fold.hidden, 1);
    assert_eq!(fold.already_planned, 1);
    assert_eq!(fold.sub_projects, 1);
    let quiet = plan
        .unblocked
        .iter()
        .find(|row| row.block_id == "quiet")
        .expect("hidden resident");
    assert_eq!(quiet.not_linked, Some("hidden"));
    assert!(quiet.offer.is_none());
    let planned = plan
        .unblocked
        .iter()
        .find(|row| row.block_id == "planned")
        .expect("already-planned resident");
    assert_eq!(planned.not_linked, Some("already_planned"));
    assert!(planned.offer.is_none());
    let offer = plan
        .unblocked
        .iter()
        .find(|row| row.block_id == "offer")
        .expect("offered resident");
    assert_eq!(
        offer.offer.as_ref().map(|draft| draft.group),
        Some(OfferGroup::Project)
    );
    assert!(!offer.offer.as_ref().unwrap().selected);
    assert!(
        !plan.changed_files.contains_key(&PathBuf::from("k.md")),
        "descendants unedited: {:?}",
        plan.changed_files
    );
}

// SO20: pick freshness bytes versus an unpicked Ready recovery's WRITE
// rule.
#[test]
fn so20_pick_stamps_fresh_unpicked_writes_unblocked() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let note = concat!(
        "- [x] #task P [id:: p] ^p\n",
        "- [?] #task Keep [fresh:: 2026-10-01] [keeps:: 2] [unblocked:: 2026-10-01] [dependsOn:: p] [id:: keep] ^keep\n",
        "- [?] #task Skip [fresh:: 2026-10-01] [dependsOn:: p] [id:: skip] ^skip\n",
    );
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] () \u{2014} FIX\n",
        "  - [[sase#^p]]\n",
    );
    let plan = run_sel(
        vec![pred("sase.md", "p", "p", "P", true)],
        &[("sase.md", note)],
        Some(day),
        "20261005.md",
        true,
        &vault_index,
        SuccessorSelection::Keys(BTreeSet::from(["sase.md#^keep".to_string()])),
    );
    let updated = plan
        .changed_files
        .get(&PathBuf::from("sase.md"))
        .expect("sase.md changed");
    let keep_line = updated
        .lines()
        .find(|line| line.contains("^keep"))
        .expect("keep line");
    assert!(keep_line.contains("[fresh:: 2026-10-05]"), "{keep_line}");
    assert!(!keep_line.contains("[unblocked::"), "{keep_line}");
    assert!(!keep_line.contains("[keeps::"), "{keep_line}");
    let skip_line = updated
        .lines()
        .find(|line| line.contains("^skip"))
        .expect("skip line");
    assert!(
        skip_line.contains("[unblocked:: 2026-10-05]"),
        "{skip_line}"
    );
    assert_eq!(
        plan.unblocked
            .iter()
            .find(|row| row.block_id == "skip")
            .map(|row| row.not_linked),
        Some(Some("not_selected"))
    );
}

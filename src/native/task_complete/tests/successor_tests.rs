//! Successor-link planner tests: outcomes of the shared rule
//! (`docs/task-dependencies.md` §11.6 SL vectors) through the pure
//! planner against in-memory snapshots, in the style of
//! `recovery_tests.rs`.

use super::super::super::vault_links::NoteIndex;
use super::super::successors::{
    insert_successor_links, plan_successors, CompletedTask, SuccessorInput,
    SuccessorPlacement, SuccessorTarget,
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
    let settings = test_settings();
    plan_successors(SuccessorInput {
        completed,
        snapshot: snapshot
            .iter()
            .map(|(path, contents)| (PathBuf::from(path), contents.to_string()))
            .collect(),
        pre_day: pre_day.map(str::to_string),
        post_day: None,
        closing: None,
        day_relative: PathBuf::from(day_relative),
        today: today(),
        tasks_settings: &settings,
        note_settings: &settings,
        link_unblocked,
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
        "- [*] #task Relaunch [dependsOn:: fix-apollo] [id:: relaunch] ^relaunch"
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

// SL9: with no live link today the dependent recovers to Ready and the
// ledger is untouched.
#[test]
fn sl9_unplanned_predecessor_recovers_without_link() {
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
    assert_eq!(row.status_symbol, ' ');
    assert_eq!(row.not_linked, Some("not_planned_today"));
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

// SL17: six successors from one gesture link none; all recover with the
// breaker reason.
#[test]
fn sl17_breaker_links_none() {
    let vault_index = index(&["sase.md", "20261005.md"]);
    let mut note =
        "- [x] #task Fix apollo [id:: fix-apollo] ^fix-apollo\n".to_string();
    for n in 1..=6 {
        note.push_str(&format!(
            "- [?] #task Fan {n} [dependsOn:: fix-apollo] [id:: fan-{n}] ^fan-{n}\n"
        ));
    }
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
    assert_eq!(plan.unblocked.len(), 6);
    for row in &plan.unblocked {
        assert_eq!(row.not_linked, Some("breaker"));
        assert!(row.link.is_none());
    }
    assert!(plan.new_day_text.is_none());
}

// SL18: `plan.link_unblocked: false` recovers to the derived rank with
// no links and no mints.
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
#[test]
fn sl26_inbox_successor_links_with_flag() {
    let vault_index = index(&["sase.md", "mac_inbox.md", "20261005.md"]);
    let plan = run(
        vec![pred("sase.md", "fix-apollo", "fix-apollo", "Fix apollo", true)],
        &[(
            "mac_inbox.md",
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
    assert!(!super::super::successors::is_inbox_note_path(Path::new(
        "sase.md"
    )));
    assert!(super::super::successors::is_inbox_note_path(Path::new(
        "mac_inbox.md",
    )));
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

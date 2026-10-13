//! Project pull-in planner tests: outcomes of the shared rule
//! (`docs/task-dependencies.md` §12.10 / §12.11 PP vectors) through the pure
//! planner against in-memory snapshots, in the style of
//! `successor_tests.rs`. Every test cites its vector id.

use super::super::super::vault_links::NoteIndex;
use super::super::inspect_pull_ins;
use super::super::successors::{
    plan_successors, CompletedTask, SuccessorInput, SuccessorPlan,
};
use super::*;

fn today() -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(2026, 10, 5).expect("date")
}

fn task(path: &str, block: &str, id: &str, text: &str) -> CompletedTask {
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

#[allow(clippy::too_many_arguments)]
fn run(
    completed: Vec<CompletedTask>,
    snapshot: &[(&str, &str)],
    note_texts: &[(&str, &str)],
    pre_day: Option<&str>,
    day_relative: &str,
    link_unblocked: bool,
    vault_index: &NoteIndex,
) -> SuccessorPlan {
    let settings = test_settings();
    plan_successors(SuccessorInput {
        completed,
        snapshot: snapshot
            .iter()
            .map(|(path, contents)| (PathBuf::from(path), contents.to_string()))
            .collect(),
        note_texts: note_texts
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
        index: vault_index,
    })
}

fn project_note(body: &str) -> String {
    format!("---\ntype: \"[[project]]\"\nstatus: wip\n---\n{body}")
}

fn day_fix(link_bullets: &str) -> String {
    format!("## Pomodoros\n- [ ] () — FIX\n{link_bullets}")
}

fn staged_closed_a(r_line: &str) -> String {
    project_note(&format!(
        "- [x] #task Fix pump [id:: fix-a] ^a-fix\n{r_line}\n"
    ))
}

fn closed_a() -> CompletedTask {
    task("proj.md", "a-fix", "fix-a", "Fix pump")
}

// PP1: N has one open task A, linked in FIX; R is ` ` with `#hide`; close
// A → R is linked right after A's bullet; ` `→`*`; `#hide` removed;
// `trigger: project_empty`; `hide_removed: true`; `unblocked_by: [A]`.
#[test]
fn pp1_last_task_close_pulls_in_prj() {
    let vault_index = index(&["proj.md", "20261005.md"]);
    let pre_day = day_fix("  - [[proj#^a-fix]]\n");
    let plan = run(
        vec![closed_a()],
        &[],
        &[(
            "proj.md",
            &staged_closed_a("- [ ] #task #prj Ship it! #hide ^prj"),
        )],
        Some(&pre_day),
        "20261005.md",
        true,
        &vault_index,
    );
    assert!(plan.ran_lookup);
    assert!(plan.still_blocked.is_empty());
    assert_eq!(plan.unblocked.len(), 1);
    let row = &plan.unblocked[0];
    assert_eq!(row.block_id, "prj");
    assert_eq!(row.previous_status_symbol, ' ');
    assert_eq!(row.status_symbol, '*');
    assert_eq!(row.trigger, "project_empty");
    assert!(row.hide_removed);
    assert_eq!(row.text, "Ship it!");
    assert_eq!(row.not_linked, None);
    assert_eq!(row.unblocked_by.len(), 1);
    assert_eq!(row.unblocked_by[0].block_id, "a-fix");
    let link = row.link.as_ref().expect("linked");
    assert_eq!(link.entry_name, "FIX");
    assert_eq!(link.block_link, "[[proj#^prj]]");
    assert!(!link.block_id_created);
    let updated = plan
        .changed_files
        .get(&PathBuf::from("proj.md"))
        .expect("proj.md changed");
    assert!(
        updated.contains("- [*] #task #prj Ship it! ^prj"),
        "{updated}"
    );
    assert!(!updated.contains("#hide"), "{updated}");
    let day = plan.new_day_text.expect("day changed");
    let anchor = day.find("[[proj#^a-fix]]").expect("anchor bullet");
    let pulled = day.find("[[proj#^prj]]").expect("pulled bullet");
    assert!(anchor < pulled, "{day}");
}

// PP2: A and B are open; close A → nothing.
#[test]
fn pp2_open_sibling_suppresses_pull_in() {
    let vault_index = index(&["proj.md", "20261005.md"]);
    let plan = run(
        vec![closed_a()],
        &[],
        &[(
            "proj.md",
            &project_note(concat!(
                "- [x] #task Fix pump [id:: fix-a] ^a-fix\n",
                "- [ ] #task Other work [id:: other] ^other\n",
                "- [ ] #task #prj Ship it! #hide ^prj\n",
            )),
        )],
        Some(&day_fix("  - [[proj#^a-fix]]\n")),
        "20261005.md",
        true,
        &vault_index,
    );
    assert!(plan.ran_lookup);
    assert!(plan.unblocked.is_empty());
    assert!(plan.still_blocked.is_empty());
    assert!(plan.changed_files.is_empty());
    assert!(plan.new_day_text.is_none());
}

// PP3: B is `[?]` → nothing (Blocked tasks are remaining work).
#[test]
fn pp3_blocked_sibling_suppresses_pull_in() {
    let vault_index = index(&["proj.md", "20261005.md"]);
    let plan = run(
        vec![closed_a()],
        &[],
        &[(
            "proj.md",
            &project_note(concat!(
                "- [x] #task Fix pump [id:: fix-a] ^a-fix\n",
                "- [?] #task Blocked work [id:: blocked] ^blocked\n",
                "- [ ] #task #prj Ship it! #hide ^prj\n",
            )),
        )],
        Some(&day_fix("  - [[proj#^a-fix]]\n")),
        "20261005.md",
        true,
        &vault_index,
    );
    assert!(plan.unblocked.is_empty());
    assert!(plan.still_blocked.is_empty());
    assert!(plan.changed_files.is_empty());
}

// PP4: B carries `#hide` → pull-in.
#[test]
fn pp4_hidden_sibling_pulls_in() {
    let vault_index = index(&["proj.md", "20261005.md"]);
    let plan = run(
        vec![closed_a()],
        &[],
        &[(
            "proj.md",
            &project_note(concat!(
                "- [x] #task Fix pump [id:: fix-a] ^a-fix\n",
                "- [ ] #task Hidden helper #hide ^helper\n",
                "- [ ] #task #prj Ship it! #hide ^prj\n",
            )),
        )],
        Some(&day_fix("  - [[proj#^a-fix]]\n")),
        "20261005.md",
        true,
        &vault_index,
    );
    assert_eq!(plan.unblocked.len(), 1);
    let row = &plan.unblocked[0];
    assert_eq!(row.block_id, "prj");
    assert_eq!(row.status_symbol, '*');
    assert!(row.link.is_some());
    assert_eq!(row.trigger, "project_empty");
}

// PP5: B is fenced → pull-in. B is blockquoted or custom `[>]` → nothing.
#[test]
fn pp5_fenced_sibling_pulls_in() {
    let vault_index = index(&["proj.md", "20261005.md"]);
    let plan = run(
        vec![closed_a()],
        &[],
        &[(
            "proj.md",
            &project_note(concat!(
                "- [x] #task Fix pump [id:: fix-a] ^a-fix\n",
                "```\n",
                "- [ ] #task Fenced helper ^fenced\n",
                "```\n",
                "- [ ] #task #prj Ship it! #hide ^prj\n",
            )),
        )],
        Some(&day_fix("  - [[proj#^a-fix]]\n")),
        "20261005.md",
        true,
        &vault_index,
    );
    assert_eq!(plan.unblocked.len(), 1);
    assert_eq!(plan.unblocked[0].block_id, "prj");
    assert!(plan.unblocked[0].link.is_some());
}

#[test]
fn pp5_blockquoted_and_custom_siblings_suppress_pull_in() {
    let vault_index = index(&["proj.md", "20261005.md"]);
    for sibling in [
        "> - [ ] #task Quoted helper ^quoted\n",
        "- [>] #task Custom helper ^custom\n",
    ] {
        let plan = run(
            vec![closed_a()],
            &[],
            &[(
                "proj.md",
                &project_note(&format!(
                    "- [x] #task Fix pump [id:: fix-a] ^a-fix\n\
                     {sibling}\
                     - [ ] #task #prj Ship it! #hide ^prj\n"
                )),
            )],
            Some(&day_fix("  - [[proj#^a-fix]]\n")),
            "20261005.md",
            true,
            &vault_index,
        );
        assert!(plan.unblocked.is_empty(), "{sibling}");
        assert!(plan.still_blocked.is_empty(), "{sibling}");
        assert!(plan.changed_files.is_empty(), "{sibling}");
    }
}

// PP6: a Sub-projects line with an unstruck child, or `~~[[child]]~~`
// struck without ✅/❌, reads as open → nothing. Only ✅/❌ children →
// pull-in.
#[test]
fn pp6_open_subproject_marker_suppresses_pull_in() {
    let vault_index = index(&["proj.md", "20261005.md"]);
    for marker in [
        "  - 🧩 **Sub-projects:** [[child]]\n",
        "  - 🧩 **Sub-projects:** ~~[[child]]~~\n",
    ] {
        let plan = run(
            vec![closed_a()],
            &[],
            &[(
                "proj.md",
                &project_note(&format!(
                    "- [x] #task Fix pump [id:: fix-a] ^a-fix\n\
                     - [ ] #task #prj Ship it! #hide ^prj\n\
                     {marker}"
                )),
            )],
            Some(&day_fix("  - [[proj#^a-fix]]\n")),
            "20261005.md",
            true,
            &vault_index,
        );
        assert!(plan.unblocked.is_empty(), "{marker}");
        assert!(plan.still_blocked.is_empty(), "{marker}");
        assert!(plan.changed_files.is_empty(), "{marker}");
    }
}

#[test]
fn pp6_closed_subproject_marker_pulls_in() {
    let vault_index = index(&["proj.md", "20261005.md"]);
    for marker in [
        "  - 🧩 **Sub-projects:** ~~[[child]]~~ ✅\n",
        "  - 🧩 **Sub-projects:** ~~[[child]]~~ ❌\n",
    ] {
        let plan = run(
            vec![closed_a()],
            &[],
            &[(
                "proj.md",
                &project_note(&format!(
                    "- [x] #task Fix pump [id:: fix-a] ^a-fix\n\
                     - [ ] #task #prj Ship it! #hide ^prj\n\
                     {marker}"
                )),
            )],
            Some(&day_fix("  - [[proj#^a-fix]]\n")),
            "20261005.md",
            true,
            &vault_index,
        );
        assert_eq!(plan.unblocked.len(), 1, "{marker}");
        assert_eq!(plan.unblocked[0].block_id, "prj");
        assert!(plan.unblocked[0].link.is_some(), "{marker}");
    }
}

// PP7: future project `scheduled` on `^prj` → `still_blocked{reason:
// scheduled}` with the date; `trigger: project_empty`; R byte-identical.
#[test]
fn pp7_future_schedule_stays_blocked() {
    let vault_index = index(&["proj.md", "20261005.md"]);
    let staged = staged_closed_a(
        "- [ ] #task #prj Ship it! #hide [scheduled:: 2026-10-13] ^prj",
    );
    let plan = run(
        vec![closed_a()],
        &[],
        &[("proj.md", &staged)],
        Some(&day_fix("  - [[proj#^a-fix]]\n")),
        "20261005.md",
        true,
        &vault_index,
    );
    assert!(plan.unblocked.is_empty());
    assert_eq!(plan.still_blocked.len(), 1);
    let row = &plan.still_blocked[0];
    assert_eq!(row.block_id, "prj");
    assert_eq!(row.reason, "scheduled");
    assert_eq!(row.scheduled.as_deref(), Some("2026-10-13"));
    assert_eq!(row.project, Some(PathBuf::from("proj.md")));
    assert_eq!(row.trigger, "project_empty");
    assert_eq!(row.text, "Ship it! [scheduled:: 2026-10-13]");
    assert!(plan.changed_files.is_empty());
    assert!(plan.new_day_text.is_none());
}

// PP8: R has an open prerequisite on its Depends-On line → still_blocked;
// R byte-identical.
#[test]
fn pp8_open_prerequisite_stays_blocked() {
    let vault_index = index(&["pnote.md", "proj.md", "20261005.md"]);
    let plan = run(
        vec![closed_a()],
        &[("pnote.md", "- [ ] #task Pay vendor [id:: pay] ^pay\n")],
        &[(
            "proj.md",
            &project_note(concat!(
                "- [x] #task Fix pump [id:: fix-a] ^a-fix\n",
                "- [ ] #task #prj Ship it! #hide [dependsOn:: pay] ^prj\n",
            )),
        )],
        Some(&day_fix("  - [[proj#^a-fix]]\n")),
        "20261005.md",
        true,
        &vault_index,
    );
    assert!(plan.unblocked.is_empty());
    assert_eq!(plan.still_blocked.len(), 1);
    let row = &plan.still_blocked[0];
    assert_eq!(row.block_id, "prj");
    assert_eq!(row.reason, "waits_on");
    assert_eq!(row.waits_on, 1);
    assert_eq!(row.project, Some(PathBuf::from("proj.md")));
    assert_eq!(row.trigger, "project_empty");
    assert!(plan.changed_files.is_empty());
}

// PP9: R already linked today → no new link; ` `→`*`; `#hide` removed;
// `already_planned`.
#[test]
fn pp9_already_linked_recovers_planned() {
    let vault_index = index(&["proj.md", "20261005.md"]);
    let plan = run(
        vec![closed_a()],
        &[],
        &[(
            "proj.md",
            &staged_closed_a("- [ ] #task #prj Ship it! #hide ^prj"),
        )],
        Some(&day_fix("  - [[proj#^a-fix]]\n  - [[proj#^prj]]\n")),
        "20261005.md",
        true,
        &vault_index,
    );
    assert!(plan.still_blocked.is_empty());
    assert_eq!(plan.unblocked.len(), 1);
    let row = &plan.unblocked[0];
    assert_eq!(row.block_id, "prj");
    assert_eq!(row.previous_status_symbol, ' ');
    assert_eq!(row.status_symbol, '*');
    assert_eq!(row.not_linked, Some("already_planned"));
    assert_eq!(row.link, None);
    assert_eq!(row.trigger, "project_empty");
    assert!(row.hide_removed);
    let updated = plan
        .changed_files
        .get(&PathBuf::from("proj.md"))
        .expect("proj.md changed");
    assert!(
        updated.contains("- [*] #task #prj Ship it! ^prj"),
        "{updated}"
    );
}

// PP10: A not planned today → no link; status unchanged; `#hide` removed;
// `not_planned_today`; row reported (verb `surfaced`).
#[test]
fn pp10_unplanned_predecessor_surfaces() {
    let vault_index = index(&["proj.md", "20261005.md"]);
    let plan = run(
        vec![closed_a()],
        &[],
        &[(
            "proj.md",
            &staged_closed_a("- [ ] #task #prj Ship it! #hide ^prj"),
        )],
        Some(&day_fix("  - [[other#^x]]\n")),
        "20261005.md",
        true,
        &vault_index,
    );
    assert!(plan.still_blocked.is_empty());
    assert_eq!(plan.unblocked.len(), 1);
    let row = &plan.unblocked[0];
    assert_eq!(row.block_id, "prj");
    assert_eq!(row.previous_status_symbol, ' ');
    assert_eq!(row.status_symbol, ' ');
    assert_eq!(row.not_linked, Some("not_planned_today"));
    assert_eq!(row.link, None);
    assert_eq!(row.trigger, "project_empty");
    assert!(row.hide_removed);
    let updated = plan
        .changed_files
        .get(&PathBuf::from("proj.md"))
        .expect("proj.md changed");
    assert!(
        updated.contains("- [ ] #task #prj Ship it! ^prj"),
        "{updated}"
    );
    assert!(plan.new_day_text.is_none());
}

// PP11: `plan.link_unblocked: false` → as PP10 with `disabled`.
#[test]
fn pp11_disabled_link_surfaces() {
    let vault_index = index(&["proj.md", "20261005.md"]);
    let plan = run(
        vec![closed_a()],
        &[],
        &[(
            "proj.md",
            &staged_closed_a("- [ ] #task #prj Ship it! #hide ^prj"),
        )],
        Some(&day_fix("  - [[proj#^a-fix]]\n")),
        "20261005.md",
        false,
        &vault_index,
    );
    assert_eq!(plan.unblocked.len(), 1);
    let row = &plan.unblocked[0];
    assert_eq!(row.not_linked, Some("disabled"));
    assert_eq!(row.previous_status_symbol, ' ');
    assert_eq!(row.status_symbol, ' ');
    assert!(row.hide_removed);
    assert_eq!(row.trigger, "project_empty");
}

// PP12: R is `[/]` → linked; stays `[/]`.
#[test]
fn pp12_in_progress_prj_links_unchanged() {
    let vault_index = index(&["proj.md", "20261005.md"]);
    let plan = run(
        vec![closed_a()],
        &[],
        &[(
            "proj.md",
            &staged_closed_a("- [/] #task #prj Ship it! #hide ^prj"),
        )],
        Some(&day_fix("  - [[proj#^a-fix]]\n")),
        "20261005.md",
        true,
        &vault_index,
    );
    assert_eq!(plan.unblocked.len(), 1);
    let row = &plan.unblocked[0];
    assert_eq!(row.previous_status_symbol, '/');
    assert_eq!(row.status_symbol, '/');
    assert!(row.link.is_some());
    assert!(row.hide_removed);
    let updated = plan
        .changed_files
        .get(&PathBuf::from("proj.md"))
        .expect("proj.md changed");
    assert!(
        updated.contains("- [/] #task #prj Ship it! ^prj"),
        "{updated}"
    );
}

// PP14: A has no `[id::]` → the gate does not suppress the pull-in, and
// the dependents snapshot is not built (empty snapshot here).
#[test]
fn pp14_id_less_close_pulls_in() {
    let vault_index = index(&["proj.md", "20261005.md"]);
    let staged = project_note(concat!(
        "- [x] #task Fix pump ^a-fix\n",
        "- [ ] #task #prj Ship it! #hide ^prj\n",
    ));
    let completed = vec![task("proj.md", "a-fix", "", "Fix pump")];
    let settings = test_settings();
    assert!(
        !inspect_pull_ins(
            &completed,
            &[(PathBuf::from("proj.md"), staged.clone())],
            &settings,
            today(),
        )
        .needs_dependents,
        "no snapshot needed without ids or R dependsOn"
    );
    let plan = run(
        completed,
        &[],
        &[("proj.md", &staged)],
        Some(&day_fix("  - [[proj#^a-fix]]\n")),
        "20261005.md",
        true,
        &vault_index,
    );
    assert!(plan.ran_lookup);
    assert_eq!(plan.unblocked.len(), 1);
    let row = &plan.unblocked[0];
    assert_eq!(row.block_id, "prj");
    assert_eq!(row.status_symbol, '*');
    assert!(row.link.is_some());
    assert_eq!(row.trigger, "project_empty");
    assert_eq!(row.unblocked_by.len(), 1);
    assert_eq!(row.unblocked_by[0].block_id, "a-fix");
}

// PP15: a dependency successor D and R in one gesture → both linked after
// A's bullet in ORDER. With six dependents plus R → six `breaker` rows,
// and R still linked.
#[test]
fn pp15_dependent_and_pull_in_link_together() {
    let vault_index = index(&["n-proj.md", "sase.md", "20261005.md"]);
    let staged = project_note(concat!(
        "- [x] #task Fix pump [id:: fix-a] ^a-fix\n",
        "- [ ] #task #prj Ship it! #hide ^prj\n",
    ));
    let plan = run(
        vec![task("n-proj.md", "a-fix", "fix-a", "Fix pump")],
        &[(
            "sase.md",
            "- [?] #task Relaunch [dependsOn:: fix-a] ^relaunch\n",
        )],
        &[("n-proj.md", &staged)],
        Some(&day_fix("  - [[n-proj#^a-fix]]\n")),
        "20261005.md",
        true,
        &vault_index,
    );
    assert_eq!(plan.unblocked.len(), 2);
    for row in &plan.unblocked {
        assert!(row.link.is_some(), "{row:?}");
    }
    let pull_in = plan
        .unblocked
        .iter()
        .find(|row| row.block_id == "prj")
        .expect("pull-in row");
    assert_eq!(pull_in.trigger, "project_empty");
    let dependent = plan
        .unblocked
        .iter()
        .find(|row| row.block_id == "relaunch")
        .expect("dependent row");
    assert_eq!(dependent.trigger, "dependency");
    let day = plan.new_day_text.expect("day changed");
    let pulled = day.find("[[n-proj#^prj]]").expect("pulled bullet");
    let dep = day.find("[[sase#^relaunch]]").expect("dependent bullet");
    assert!(pulled < dep, "{day}");
}

#[test]
fn pp15_breaker_spares_pull_in() {
    let vault_index = index(&["n-proj.md", "sase.md", "20261005.md"]);
    let staged = project_note(concat!(
        "- [x] #task Fix pump [id:: fix-a] ^a-fix\n",
        "- [ ] #task #prj Ship it! #hide ^prj\n",
    ));
    let mut dependents = String::new();
    for n in 1..=6 {
        dependents.push_str(&format!(
            "- [?] #task Dependent {n} [dependsOn:: fix-a] ^dep-{n}\n"
        ));
    }
    let plan = run(
        vec![task("n-proj.md", "a-fix", "fix-a", "Fix pump")],
        &[("sase.md", &dependents)],
        &[("n-proj.md", &staged)],
        Some(&day_fix("  - [[n-proj#^a-fix]]\n")),
        "20261005.md",
        true,
        &vault_index,
    );
    assert_eq!(plan.unblocked.len(), 7);
    let pull_in = plan
        .unblocked
        .iter()
        .find(|row| row.block_id == "prj")
        .expect("pull-in row");
    assert!(pull_in.link.is_some());
    assert_eq!(pull_in.trigger, "project_empty");
    let broken: Vec<_> = plan
        .unblocked
        .iter()
        .filter(|row| row.block_id != "prj")
        .collect();
    assert_eq!(broken.len(), 6);
    for row in broken {
        assert_eq!(row.not_linked, Some("breaker"), "{row:?}");
    }
    let updated = plan
        .changed_files
        .get(&PathBuf::from("n-proj.md"))
        .expect("n-proj.md changed");
    assert!(
        updated.contains("- [*] #task #prj Ship it! ^prj"),
        "{updated}"
    );
}

// PP17: R depends on A through a legacy internal Depends-On line → one
// row, and the pull-in wins over `project_task`.
#[test]
fn pp17_pull_in_wins_over_project_task() {
    let vault_index = index(&["proj.md", "20261005.md"]);
    let staged = project_note(concat!(
        "- [x] #task Fix pump [id:: fix-a] ^a-fix\n",
        "- [ ] #task #prj Ship it! #hide [dependsOn:: fix-a] ^prj\n",
        "  - ⛓️ **DEPENDS ON:** [[#^a-fix]]\n",
    ));
    let plan = run(
        vec![closed_a()],
        &[],
        &[("proj.md", &staged)],
        Some(&day_fix("  - [[proj#^a-fix]]\n")),
        "20261005.md",
        true,
        &vault_index,
    );
    assert_eq!(plan.unblocked.len(), 1);
    let row = &plan.unblocked[0];
    assert_eq!(row.block_id, "prj");
    assert!(row.link.is_some());
    assert_eq!(row.trigger, "project_empty");
    assert!(plan.still_blocked.is_empty());
}

// PP20: terminal project, two `^prj`, N under `done/`, a canonical daily
// note, or N with a sync scan issue (two inline dates on `^prj`) → nothing.
#[test]
fn pp20_unplannable_notes_stay_silent() {
    let cases: Vec<(&str, String)> = vec![
        ("proj.md", {
            let mut note = project_note(concat!(
                "- [x] #task Fix pump [id:: fix-a] ^a-fix\n",
                "- [ ] #task #prj Ship it! #hide ^prj\n",
            ));
            note = note.replacen("status: wip", "status: done", 1);
            note
        }),
        ("proj.md", project_note(concat!(
            "- [x] #task Fix pump [id:: fix-a] ^a-fix\n",
            "- [ ] #task #prj Ship it! #hide ^prj\n",
            "- [ ] #task #prj Second criteria #hide ^prj\n",
        ))),
        ("done/proj.md", staged_closed_a("- [ ] #task #prj Ship it! #hide ^prj")),
        ("2026/20261005.md", staged_closed_a("- [ ] #task #prj Ship it! #hide ^prj")),
        ("proj.md", staged_closed_a(
            "- [ ] #task #prj Ship it! #hide [scheduled:: 2026-10-06] [scheduled:: 2026-10-07] ^prj",
        )),
    ];
    for (path, staged) in &cases {
        let vault_index = index(&[path, "20261005.md"]);
        let completed = vec![CompletedTask {
            relative_path: PathBuf::from(*path),
            ..closed_a()
        }];
        let plan = run(
            completed,
            &[],
            &[(path, staged.as_str())],
            Some(&day_fix("  - [[proj#^a-fix]]\n")),
            "20261005.md",
            true,
            &vault_index,
        );
        assert!(plan.unblocked.is_empty(), "{path}");
        assert!(plan.still_blocked.is_empty(), "{path}");
        assert!(plan.changed_files.is_empty(), "{path}");
        assert!(plan.new_day_text.is_none(), "{path}");
    }
}

// PP22: `#hide` bytes via `normalize_task_hide_tag(line, false, true)`.
#[test]
fn pp22_hide_strip_bytes() {
    use super::super::super::projects::edits::normalize_task_hide_tag;
    for (line, expected) in [
        (
            "- [ ] #task #prj Ship it! #hide ^prj",
            "- [ ] #task #prj Ship it! ^prj",
        ),
        (
            "- [ ] #task #prj Ship it! #hide [priority:: high] ^prj",
            "- [ ] #task #prj Ship it! [priority:: high] ^prj",
        ),
        (
            "- [ ] #task #prj Ship it! #hidden #hide ^prj",
            "- [ ] #task #prj Ship it! #hidden ^prj",
        ),
    ] {
        assert_eq!(normalize_task_hide_tag(line, false, true), expected);
    }
}

// PP24: closing P in another note when R depended only on P and N is empty
// → R pulled in after P's bullet. If N is not empty → `project_task`
// (unchanged).
#[test]
fn pp24_cross_note_dependent_pulls_in() {
    let vault_index = index(&["pnote.md", "proj.md", "20261005.md"]);
    let plan = run(
        vec![task("pnote.md", "pay", "pay", "Pay vendor")],
        &[
            ("pnote.md", "- [x] #task Pay vendor [id:: pay] ^pay\n"),
            (
                "proj.md",
                &project_note(
                    "- [ ] #task #prj Ship it! #hide [dependsOn:: pay] ^prj\n",
                ),
            ),
        ],
        &[],
        Some(&day_fix("  - [[pnote#^pay]]\n")),
        "20261005.md",
        true,
        &vault_index,
    );
    assert!(plan.still_blocked.is_empty());
    assert_eq!(plan.unblocked.len(), 1);
    let row = &plan.unblocked[0];
    assert_eq!(row.block_id, "prj");
    assert_eq!(row.previous_status_symbol, ' ');
    assert_eq!(row.status_symbol, '*');
    assert_eq!(row.trigger, "project_empty");
    assert!(row.hide_removed);
    assert_eq!(row.text, "Ship it! [dependsOn:: pay]");
    assert_eq!(row.unblocked_by.len(), 1);
    assert_eq!(row.unblocked_by[0].block_id, "pay");
    let link = row.link.as_ref().expect("linked");
    assert_eq!(link.entry_name, "FIX");
    assert_eq!(link.block_link, "[[proj#^prj]]");
    let updated = plan
        .changed_files
        .get(&PathBuf::from("proj.md"))
        .expect("proj.md changed");
    assert!(
        updated.contains("- [*] #task #prj Ship it! [dependsOn:: pay] ^prj"),
        "{updated}"
    );
}

#[test]
fn pp24_non_empty_note_keeps_project_task() {
    let vault_index = index(&["pnote.md", "proj.md", "20261005.md"]);
    let staged = project_note(concat!(
        "- [ ] #task #prj Ship it! #hide [dependsOn:: pay] ^prj\n",
        "- [ ] #task Other work ^other\n",
    ));
    let plan = run(
        vec![task("pnote.md", "pay", "pay", "Pay vendor")],
        &[
            ("pnote.md", "- [x] #task Pay vendor [id:: pay] ^pay\n"),
            ("proj.md", &staged),
        ],
        &[],
        Some(&day_fix("  - [[pnote#^pay]]\n")),
        "20261005.md",
        true,
        &vault_index,
    );
    assert!(plan.unblocked.is_empty());
    assert!(plan.still_blocked.is_empty());
    assert!(!plan.changed_files.contains_key(&PathBuf::from("proj.md")));
    assert!(plan.new_day_text.is_none());
}

// PP13: R is `[*]`, visible and already linked → no write, no row.
#[test]
fn pp13_visible_linked_next_is_silent() {
    let vault_index = index(&["proj.md", "20261005.md"]);
    let plan = run(
        vec![closed_a()],
        &[],
        &[(
            "proj.md",
            &staged_closed_a("- [*] #task #prj Ship it! ^prj"),
        )],
        Some(&day_fix("  - [[proj#^a-fix]]\n  - [[proj#^prj]]\n")),
        "20261005.md",
        true,
        &vault_index,
    );
    assert!(plan.unblocked.is_empty());
    assert!(plan.still_blocked.is_empty());
    assert!(plan.changed_files.is_empty());
}

// PP25: R is `[?] … #hide` and depends only on P in another note. N's only
// other open task is `[?] #prj_is_blocked #hide`. Close P (planned today) →
// R is pulled in after P's bullet: `[?]`→`[*]`, R's `#hide` removed. The
// resident loses `#prj_is_blocked` and recovers to `[ ]` as
// `project_member`. It gets no `[unblocked::]` (it is still hidden, UB6).
#[test]
fn pp25_blocked_prj_pull_in_recovers_hidden_resident() {
    let vault_index = index(&["pnote.md", "proj.md", "20261005.md"]);
    let staged = project_note(concat!(
        "- [?] #task #prj Ship it! #hide [dependsOn:: pay] ^prj\n",
        "- [?] #task Help out #prj_is_blocked #hide ^help\n",
    ));
    let plan = run(
        vec![task("pnote.md", "pay", "pay", "Pay vendor")],
        &[
            ("pnote.md", "- [x] #task Pay vendor [id:: pay] ^pay\n"),
            ("proj.md", &staged),
        ],
        &[],
        Some(&day_fix("  - [[pnote#^pay]]\n")),
        "20261005.md",
        true,
        &vault_index,
    );
    assert!(plan.still_blocked.is_empty());
    assert_eq!(plan.unblocked.len(), 2);
    let pulled = plan
        .unblocked
        .iter()
        .find(|row| row.block_id == "prj")
        .expect("pull-in row");
    assert_eq!(pulled.previous_status_symbol, '?');
    assert_eq!(pulled.status_symbol, '*');
    assert!(pulled.link.is_some());
    assert_eq!(pulled.trigger, "project_empty");
    assert!(pulled.hide_removed);
    let resident = plan
        .unblocked
        .iter()
        .find(|row| row.block_id == "help")
        .expect("resident row");
    assert_eq!(resident.previous_status_symbol, '?');
    assert_eq!(resident.status_symbol, ' ');
    assert_eq!(resident.not_linked, Some("project_member"));
    assert_eq!(resident.link, None);
    assert_eq!(resident.trigger, "dependency");
    let updated = plan
        .changed_files
        .get(&PathBuf::from("proj.md"))
        .expect("proj.md changed");
    assert!(
        updated.contains("- [*] #task #prj Ship it! [dependsOn:: pay] ^prj"),
        "{updated}"
    );
    let resident_line = updated
        .lines()
        .find(|line| line.contains("^help"))
        .expect("resident line");
    assert!(resident_line.starts_with("- [ ] "), "{resident_line}");
    assert!(
        !resident_line.contains("#prj_is_blocked"),
        "{resident_line}"
    );
    assert!(!resident_line.contains("[unblocked::"), "{resident_line}");
    assert!(!updated.contains("[unblocked::"), "{updated}");
}

// PP26: R carries `[scheduled:: <today or past>]`; N's only other open task
// B carries `#hide`. Close A → nothing (B is remaining work under the dated
// recount). With no project date → pull-in.
#[test]
fn pp26_dated_recount_keeps_hidden_sibling() {
    let vault_index = index(&["proj.md", "20261005.md"]);
    let staged = project_note(concat!(
        "- [x] #task Fix pump [id:: fix-a] ^a-fix\n",
        "- [ ] #task Other work #hide ^other\n",
        "- [ ] #task #prj Ship it! #hide [scheduled:: 2026-10-05] ^prj\n",
    ));
    let plan = run(
        vec![closed_a()],
        &[],
        &[("proj.md", &staged)],
        Some(&day_fix("  - [[proj#^a-fix]]\n")),
        "20261005.md",
        true,
        &vault_index,
    );
    assert!(plan.unblocked.is_empty());
    assert!(plan.still_blocked.is_empty());
    assert!(plan.changed_files.is_empty());
}

#[test]
fn pp26_undated_hidden_sibling_pulls_in() {
    let vault_index = index(&["proj.md", "20261005.md"]);
    let staged = project_note(concat!(
        "- [x] #task Fix pump [id:: fix-a] ^a-fix\n",
        "- [ ] #task Other work #hide ^other\n",
        "- [ ] #task #prj Ship it! #hide ^prj\n",
    ));
    let plan = run(
        vec![closed_a()],
        &[],
        &[("proj.md", &staged)],
        Some(&day_fix("  - [[proj#^a-fix]]\n")),
        "20261005.md",
        true,
        &vault_index,
    );
    assert_eq!(plan.unblocked.len(), 1);
    assert_eq!(plan.unblocked[0].block_id, "prj");
    assert!(plan.unblocked[0].link.is_some());
}

// PP27: R `[?]` with `[fresh::]`, not planned today, last blocker closed →
// `' '`, `#hide` removed, `[unblocked:: today]` written,
// `not_planned_today`. Linked instead → `[*]` and no `[unblocked::]` (UB4).
#[test]
fn pp27_unplanned_blocked_prj_writes_unblocked() {
    let vault_index = index(&["pnote.md", "proj.md", "20261005.md"]);
    let staged = project_note(
        "- [?] #task #prj Ship it! #hide [fresh:: 2026-10-01] [dependsOn:: pay] ^prj\n",
    );
    let plan = run(
        vec![task("pnote.md", "pay", "pay", "Pay vendor")],
        &[
            ("pnote.md", "- [x] #task Pay vendor [id:: pay] ^pay\n"),
            ("proj.md", &staged),
        ],
        &[],
        Some(&day_fix("  - [[other#^x]]\n")),
        "20261005.md",
        true,
        &vault_index,
    );
    assert!(plan.still_blocked.is_empty());
    assert_eq!(plan.unblocked.len(), 1);
    let row = &plan.unblocked[0];
    assert_eq!(row.block_id, "prj");
    assert_eq!(row.previous_status_symbol, '?');
    assert_eq!(row.status_symbol, ' ');
    assert_eq!(row.not_linked, Some("not_planned_today"));
    assert_eq!(row.trigger, "project_empty");
    assert!(row.hide_removed);
    let updated = plan
        .changed_files
        .get(&PathBuf::from("proj.md"))
        .expect("proj.md changed");
    assert!(!updated.contains("#hide"), "{updated}");
    assert!(updated.contains("[unblocked:: 2026-10-05]"), "{updated}");
}

#[test]
fn pp27_linked_blocked_prj_skips_unblocked() {
    let vault_index = index(&["pnote.md", "proj.md", "20261005.md"]);
    let staged = project_note(
        "- [?] #task #prj Ship it! #hide [fresh:: 2026-10-01] [dependsOn:: pay] ^prj\n",
    );
    let plan = run(
        vec![task("pnote.md", "pay", "pay", "Pay vendor")],
        &[
            ("pnote.md", "- [x] #task Pay vendor [id:: pay] ^pay\n"),
            ("proj.md", &staged),
        ],
        &[],
        Some(&day_fix("  - [[pnote#^pay]]\n")),
        "20261005.md",
        true,
        &vault_index,
    );
    assert_eq!(plan.unblocked.len(), 1);
    let row = &plan.unblocked[0];
    assert_eq!(row.status_symbol, '*');
    assert!(row.link.is_some());
    let updated = plan
        .changed_files
        .get(&PathBuf::from("proj.md"))
        .expect("proj.md changed");
    assert!(!updated.contains("[unblocked::"), "{updated}");
    assert!(!updated.contains("#hide"), "{updated}");
}

// PP28: a leftover frontmatter future `scheduled` on N with an open `^prj`
// → `still_blocked{reason: scheduled}`, R byte-identical.
#[test]
fn pp28_frontmatter_schedule_stays_blocked() {
    let vault_index = index(&["proj.md", "20261005.md"]);
    let staged = concat!(
        "---\ntype: \"[[project]]\"\nstatus: wip\nscheduled: 2026-10-13\n---\n",
        "- [x] #task Fix pump [id:: fix-a] ^a-fix\n",
        "- [ ] #task #prj Ship it! #hide ^prj\n",
    );
    let plan = run(
        vec![closed_a()],
        &[],
        &[("proj.md", staged)],
        Some(&day_fix("  - [[proj#^a-fix]]\n")),
        "20261005.md",
        true,
        &vault_index,
    );
    assert!(plan.unblocked.is_empty());
    assert_eq!(plan.still_blocked.len(), 1);
    let row = &plan.still_blocked[0];
    assert_eq!(row.block_id, "prj");
    assert_eq!(row.reason, "scheduled");
    assert_eq!(row.scheduled.as_deref(), Some("2026-10-13"));
    assert_eq!(row.project, Some(PathBuf::from("proj.md")));
    assert_eq!(row.trigger, "project_empty");
    assert!(plan.changed_files.is_empty());
}

// Gate: the dependents snapshot is only needed when some C task has
// `[id::]`, or a trigger-(a) candidate R carries `[dependsOn::]` (§12.10).
#[test]
fn gate_needs_dependents_cases() {
    let settings = test_settings();
    let staged_plain = staged_closed_a("- [ ] #task #prj Ship it! #hide ^prj");
    let staged_depends = staged_closed_a(
        "- [ ] #task #prj Ship it! #hide [dependsOn:: fix-a] ^prj",
    );
    // Some C task has `[id::]` → snapshot needed.
    assert!(
        inspect_pull_ins(
            &[closed_a()],
            &[(PathBuf::from("proj.md"), staged_plain.clone())],
            &settings,
            today(),
        )
        .needs_dependents
    );
    // Id-less C, R without `dependsOn` (PP14) → no snapshot.
    assert!(
        !inspect_pull_ins(
            &[task("proj.md", "a-fix", "", "Fix pump")],
            &[(PathBuf::from("proj.md"), staged_plain.clone())],
            &settings,
            today(),
        )
        .needs_dependents
    );
    // Id-less C, R with `dependsOn` → snapshot needed for steps 1 and 3.
    assert!(
        inspect_pull_ins(
            &[task("proj.md", "a-fix", "", "Fix pump")],
            &[(PathBuf::from("proj.md"), staged_depends.clone())],
            &settings,
            today(),
        )
        .needs_dependents
    );
    // Id-less C, no staged texts → no snapshot.
    assert!(
        !inspect_pull_ins(
            &[task("proj.md", "a-fix", "", "Fix pump")],
            &[],
            &settings,
            today()
        )
        .needs_dependents
    );
}

// R1 (PP22/PP23): punctuation-bounded `#hide` uses sync's tag boundaries in
// both the linked and the not-planned-today cases.
#[test]
fn r1_punctuation_hide_linked_and_surfaced() {
    let vault_index = index(&["proj.md", "20261005.md"]);
    // Linked: `#hide,` still counts as hidden, links, and drops from display.
    let staged = staged_closed_a("- [ ] #task #prj Ship it! #hide, ^prj");
    let plan = run(
        vec![closed_a()],
        &[],
        &[("proj.md", &staged)],
        Some(&day_fix("  - [[proj#^a-fix]]\n")),
        "20261005.md",
        true,
        &vault_index,
    );
    assert_eq!(plan.unblocked.len(), 1);
    let row = &plan.unblocked[0];
    assert_eq!(row.block_id, "prj");
    assert!(row.hide_removed);
    assert_eq!(row.text, "Ship it!");
    assert!(row.link.is_some());
    let updated = plan
        .changed_files
        .get(&PathBuf::from("proj.md"))
        .expect("proj.md changed");
    assert!(!updated.contains("#hide"), "{updated}");
    assert!(updated.contains("^prj"), "{updated}");

    // Not-planned-today: same punctuation `#hide` still surfaces.
    let plan = run(
        vec![closed_a()],
        &[],
        &[("proj.md", &staged)],
        Some(&day_fix("")),
        "20261005.md",
        true,
        &vault_index,
    );
    assert_eq!(plan.unblocked.len(), 1);
    let row = &plan.unblocked[0];
    assert_eq!(row.not_linked, Some("not_planned_today"));
    assert!(row.hide_removed);
    assert_eq!(row.text, "Ship it!");
    assert_eq!(row.previous_status_symbol, row.status_symbol);
}

// R2: a `[?]` R with an open resident carrying `[dependsOn::]` needs the
// dependents snapshot, and the resident keeps its Blocked status when its
// prerequisite is still open elsewhere.
#[test]
fn r2_stale_blocked_cascade_needs_dependents() {
    let settings = test_settings();
    let staged = project_note(concat!(
        "- [x] #task Fix pump ^a-fix\n",
        "- [?] #task #prj Ship it! #hide ^prj\n",
        "- [?] #task Help #prj_is_blocked #hide [dependsOn:: ext-id] ^help\n",
    ));
    let completed = vec![task("proj.md", "a-fix", "", "Fix pump")];
    assert!(
        inspect_pull_ins(
            &completed,
            &[(PathBuf::from("proj.md"), staged.clone())],
            &settings,
            today(),
        )
        .needs_dependents,
        "stale [?] R with dependent resident needs snapshot"
    );
    let vault_index = index(&["proj.md", "other.md", "20261005.md"]);
    let plan = run(
        completed,
        &[("other.md", "- [ ] #task External [id:: ext-id] ^ext\n")],
        &[("proj.md", &staged)],
        Some(&day_fix("  - [[proj#^a-fix]]\n")),
        "20261005.md",
        true,
        &vault_index,
    );
    let resident = plan.unblocked.iter().find(|row| row.block_id == "help");
    assert!(
        resident.is_none(),
        "resident with open prerequisite stays Blocked: {:?}",
        plan.unblocked
    );
    assert!(
        plan.still_blocked.iter().all(|row| row.block_id != "help"),
        "resident is not recovered: {:?}",
        plan.still_blocked
    );
}

// R3: `reason: "project"` still-blocked rows carry `scheduled` when the
// project date blocks (§12.5).
#[test]
fn r3_project_reason_carries_scheduled() {
    let vault_index = index(&["sase.md", "proj.md", "20261005.md"]);
    let settings = test_settings();
    let plan = plan_successors(SuccessorInput {
        completed: vec![CompletedTask {
            relative_path: PathBuf::from("sase.md"),
            block_id: "done-p".to_string(),
            task_id: "done-p".to_string(),
            text: "Done P".to_string(),
            is_root: true,
        }],
        snapshot: vec![
            (
                PathBuf::from("sase.md"),
                "- [x] #task Done P [id:: done-p] ^done-p\n".to_string(),
            ),
            (
                PathBuf::from("proj.md"),
                project_note(concat!(
                    "- [?] #task Apply [scheduled:: 2026-10-13] [id:: apply] ^prj\n",
                    "- [?] #task Resident [dependsOn:: done-p] [id:: res] ^res\n",
                )),
            ),
        ],
        note_texts: vec![],
        pre_day: Some(day_fix("  - [[sase#^done-p]]\n")),
        post_day: None,
        closing: None,
        day_relative: PathBuf::from("20261005.md"),
        today: today(),
        tasks_settings: &settings,
        note_settings: &settings,
        link_unblocked: true,
        index: &vault_index,
    });
    assert!(plan.unblocked.is_empty(), "{:?}", plan.unblocked);
    assert_eq!(plan.still_blocked.len(), 1);
    let row = &plan.still_blocked[0];
    assert_eq!(row.reason, "project");
    assert_eq!(row.scheduled.as_deref(), Some("2026-10-13"));
    assert_eq!(row.project, Some(PathBuf::from("proj.md")));
}

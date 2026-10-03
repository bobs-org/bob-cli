//! Projects sync tests except schedule tail.

use crate::support::*;
use std::fs;

#[test]
fn projects_sync_updates_status_prj_hide_tag_warns_and_is_idempotent() {
    let temp = TempDir::new("bob-cli-projects-sync");
    let vault = temp.path().join("vault");

    write_file(
        &vault.join("DoneFlip.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [x] #task Ship done #hide ^prj\n",
    );
    write_file(
        &vault.join("CancelFlip.md"),
        "---\ntype: [[project]]\nstatus: waiting\n---\n- [-] #task Stop work #hide ^prj\n",
    );
    write_file(
        &vault.join("MissingStatus.md"),
        "---\ntype: [[project]]\n---\n- [X] #task Ship missing status #hide ^prj\n",
    );
    write_file(
        &vault.join("Stalled.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish stalled #hide ^prj\n- [/] #task Secondary work #hide\n",
    );
    write_file(
        &vault.join("ZeroOpen.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish zero open #hide ^prj\n- [x] #task Already done\n",
    );
    write_file(
        &vault.join("HasUnprioritized.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish has unprioritized #hide ^prj\n- [ ] #task Needs priority\n",
    );
    write_file(
        &vault.join("MissingPriority.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish missing priority ^prj\n- [ ] #task Needs priority\n",
    );
    write_file(
        &vault.join("ExistingScheduled.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish already scheduled #hide [scheduled::2026-06-01] ^prj\n",
    );
    write_file(
        &vault.join("TerminalOpen.md"),
        "---\ntype: [[project]]\nstatus: done\n---\n- [ ] #task Finish drift #hide ^prj\n",
    );
    write_file(
        &vault.join("MissingPrj.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Needs completion task\n",
    );
    write_file(
        &vault.join("Placeholder.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task <short_project_completion_criteria_goes_here> #hide ^prj\n- [ ] #task Needs priority\n",
    );

    let dry_run_snapshot =
        fs::read_to_string(vault.join("Stalled.md")).expect("read stalled");
    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("-d")
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run bob projects sync dry-run");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected dry-run stderr:\n{}",
        format_output(&output)
    );
    let out = stdout(&output);
    assert!(
        out.contains("[dry-run] ok")
            && out.contains("would set status: waiting -> canceled")
            && out.contains("would set status: done -> wip")
            && out.contains("^prj task opened")
            && out.contains("would remove #hide from ^prj")
            && out.contains("would add #hide to ^prj")
            && out.contains("would remove [scheduled::2026-06-01] from ^prj")
            && out.contains("active project has no ^prj task")
            && out.contains("template placeholder")
            && out.contains(
                "11 projects - 4 status updated - 6 ^prj edited - 0 task schedules updated - 2 warnings"
            ),
        "unexpected dry-run output:\n{out}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("Stalled.md")).expect("read stalled"),
        dry_run_snapshot,
        "dry-run must not edit files"
    );
    assert_stdout_has_no_ansi(&output);

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("run bob projects sync");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected sync stderr:\n{}",
        format_output(&output)
    );
    let out = stdout(&output);
    assert!(
        out.contains("status: wip -> done")
            && out.contains("status: done -> wip")
            && out.contains("removed #hide from ^prj")
            && out.contains("added #hide to ^prj")
            && out.contains("removed [scheduled::2026-06-01] from ^prj")
            && out.contains(
                "11 projects - 4 status updated - 6 ^prj edited - 0 task schedules updated - 2 warnings"
            ),
        "unexpected sync output:\n{out}"
    );

    assert_eq!(
        fs::read_to_string(vault.join("DoneFlip.md")).expect("read done"),
        "---\ntype: [[project]]\nstatus: done\n---\n- [x] #task Ship done #hide ^prj\n"
    );
    assert_eq!(
        fs::read_to_string(vault.join("CancelFlip.md")).expect("read cancel"),
        "---\ntype: [[project]]\nstatus: canceled\n---\n- [-] #task Stop work #hide ^prj\n"
    );
    assert_eq!(
        fs::read_to_string(vault.join("MissingStatus.md")).expect("read missing status"),
        "---\ntype: [[project]]\nstatus: done\n---\n- [X] #task Ship missing status #hide ^prj\n"
    );
    assert_eq!(
        fs::read_to_string(vault.join("Stalled.md")).expect("read stalled"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish stalled ^prj\n- [/] #task Secondary work #hide\n"
    );
    assert_eq!(
        fs::read_to_string(vault.join("ZeroOpen.md")).expect("read zero"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish zero open ^prj\n- [x] #task Already done\n"
    );
    assert_eq!(
        fs::read_to_string(vault.join("HasUnprioritized.md")).expect("read has unprioritized"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish has unprioritized #hide ^prj\n- [ ] #task Needs priority\n"
    );
    assert_eq!(
        fs::read_to_string(vault.join("MissingPriority.md")).expect("read missing priority"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish missing priority #hide ^prj\n- [ ] #task Needs priority\n"
    );
    assert_eq!(
        fs::read_to_string(vault.join("ExistingScheduled.md")).expect("read scheduled"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish already scheduled ^prj\n"
    );
    assert_eq!(
        fs::read_to_string(vault.join("TerminalOpen.md")).expect("read terminal open"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish drift ^prj\n"
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("rerun bob projects sync");

    assert_success(&output);
    assert!(
        stdout(&output).contains(
            "11 projects - 0 status updated - 0 ^prj edited - 0 task schedules updated - 2 warnings"
        ),
        "second run should have zero actions:\n{}",
        format_output(&output)
    );
}

#[test]
fn projects_sync_hides_parent_projects_with_open_subprojects() {
    let temp = TempDir::new("bob-cli-projects-sync-subprojects");
    let vault = temp.path().join("vault");

    write_file(
        &vault.join("ParentKeep.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent keep #hide ^prj\n",
    );
    write_file(
        &vault.join("ParentAdd.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent add ^prj\n",
    );
    write_file(
        &vault.join("ChildKeep.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[ParentKeep]]\n---\n- [ ] #task Finish child #hide ^prj\n- [ ] #task Needs priority\n",
    );
    write_file(
        &vault.join("ChildAdd.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[projects/ParentAdd#Now|parent]]\n---\n- [ ] #task Finish child #hide ^prj\n- [ ] #task Needs priority\n",
    );
    write_file(
        &vault.join("ChildAlpha.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[ParentAdd]]\n---\n- [ ] #task Finish alpha #hide ^prj\n- [ ] #task Needs priority\n",
    );

    let parent_add_snapshot =
        fs::read_to_string(vault.join("ParentAdd.md")).expect("read parent");
    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--dry-run")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("run bob projects sync dry-run");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("would add #hide to ^prj  project has open sub-projects")
            && out
                .contains("would add [[ChildKeep]] to ^prj  open sub-project")
            && out.contains("would add [[ChildAdd]] to ^prj  open sub-project")
            && out
                .contains("would add [[ChildAlpha]] to ^prj  open sub-project")
            && out.contains(
                "5 projects - 0 status updated - 4 ^prj edited - 0 task schedules updated - 0 warnings"
            ),
        "unexpected dry-run output:\n{out}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("ParentAdd.md")).expect("read parent"),
        parent_add_snapshot,
        "dry-run must not edit parent"
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("run bob projects sync");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("added #hide to ^prj  project has open sub-projects")
            && out.contains("added [[ChildKeep]] to ^prj  open sub-project")
            && out.contains("added [[ChildAdd]] to ^prj  open sub-project")
            && out.contains("added [[ChildAlpha]] to ^prj  open sub-project")
            && out.contains(
                "5 projects - 0 status updated - 4 ^prj edited - 0 task schedules updated - 0 warnings"
            ),
        "unexpected sync output:\n{out}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("ParentKeep.md")).expect("read parent"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent keep #hide ^prj\n\t- 🧩 **Sub-projects:** [[ChildKeep]]\n"
    );
    assert_eq!(
        fs::read_to_string(vault.join("ParentAdd.md")).expect("read parent"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent add #hide ^prj\n\t- 🧩 **Sub-projects:** [[ChildAdd]] • [[ChildAlpha]]\n"
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("rerun bob projects sync");

    assert_success(&output);
    assert!(
        stdout(&output).contains(
            "5 projects - 0 status updated - 0 ^prj edited - 0 task schedules updated - 0 warnings"
        ),
        "second run should have zero actions:\n{}",
        format_output(&output)
    );
}

#[test]
fn projects_sync_reconciles_future_subproject_markers_at_date_boundary() {
    let temp = TempDir::new("bob-cli-projects-sync-future-subprojects");
    let vault = temp.path().join("vault");

    write_file(
        &vault.join("Parent.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[FutureOpen]] • [[Today]] • [[Unscheduled]] • ~~[[FutureClosed]]~~ ✅\n\t- user-owned context\n",
    );
    write_file(
        &vault.join("Unscheduled.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[Parent]]\n---\n- [ ] #task Finish unscheduled ^prj\n",
    );
    write_file(
        &vault.join("Today.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[Parent]]\nscheduled: 2026-07-10\n---\n- [ ] #task Finish today ^prj\n",
    );
    write_file(
        &vault.join("FutureOpen.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[Parent]]\nscheduled: 2026-07-11\n---\n- [ ] #task Finish future #hide ^prj\n",
    );
    write_file(
        &vault.join("FutureClosed.md"),
        "---\ntype: [[project]]\nstatus: done\nparent: [[Parent]]\nscheduled: 2026-07-11\n---\n- [x] #task Finish closed #hide ^prj\n",
    );

    let parent_before =
        fs::read_to_string(vault.join("Parent.md")).expect("read parent");
    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--dry-run")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_NOW", "2026-07-10")
        .output()
        .expect("preview future sub-project marker sync");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains(
            "would add 🗓️ [[FutureOpen]] to ^prj  sub-project scheduled in future"
        ) && out.contains(
            "would add 🗓️ [[FutureClosed]] to ^prj  sub-project scheduled in future"
        ) && out.contains(
            "5 projects - 0 status updated - 2 ^prj edited - 0 task schedules updated - 0 warnings"
        ),
        "unexpected dry-run output:\n{out}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("Parent.md")).expect("read parent"),
        parent_before,
        "dry-run must not edit the parent ledger"
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_NOW", "2026-07-10")
        .output()
        .expect("apply future sub-project markers");
    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("added 🗓️ [[FutureOpen]] to ^prj  sub-project scheduled in future")
            && out.contains("added 🗓️ [[FutureClosed]] to ^prj  sub-project scheduled in future"),
        "unexpected sync output:\n{out}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("Parent.md")).expect("read parent"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent #hide ^prj\n\t- 🧩 **Sub-projects:** 🗓️ [[FutureOpen]] • [[Today]] • [[Unscheduled]] • 🗓️ ~~[[FutureClosed]]~~ ✅\n\t- user-owned context\n"
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_NOW", "2026-07-10")
        .output()
        .expect("rerun future sub-project marker sync");
    assert_success(&output);
    assert!(
        stdout(&output).contains(
            "5 projects - 0 status updated - 0 ^prj edited - 0 task schedules updated - 0 warnings"
        ),
        "second run should be a no-op:\n{}",
        format_output(&output)
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_NOW", "2026-07-11")
        .output()
        .expect("sync at scheduled date boundary");
    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains(
            "removed 🗓️ [[FutureOpen]] from ^prj  sub-project no longer scheduled in future"
        ) && out.contains(
            "removed 🗓️ [[FutureClosed]] from ^prj  sub-project no longer scheduled in future"
        ) && out.contains(
            "FutureOpen  removed #hide from ^prj  no non-hidden open tasks or open sub-projects"
        ) && out.contains(
            "5 projects - 0 status updated - 3 ^prj edited - 0 task schedules updated - 0 warnings"
        ),
        "unexpected boundary output:\n{out}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("Parent.md")).expect("read parent"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[FutureOpen]] • [[Today]] • [[Unscheduled]] • ~~[[FutureClosed]]~~ ✅\n\t- user-owned context\n"
    );
    // A today-scheduled empty project surfaces through the normal rule.
    assert_eq!(
        fs::read_to_string(vault.join("FutureOpen.md")).expect("read child"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[Parent]]\nscheduled: 2026-07-11\n---\n- [ ] #task Finish future ^prj\n"
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_NOW", "2026-07-11")
        .output()
        .expect("rerun scheduled date boundary sync");
    assert_success(&output);
    assert!(
        stdout(&output).contains(
            "5 projects - 0 status updated - 0 ^prj edited - 0 task schedules updated - 0 warnings"
        ),
        "boundary rerun should be a no-op:\n{}",
        format_output(&output)
    );
}

#[test]
fn projects_sync_unhides_parent_when_child_prj_is_checked_same_run() {
    let temp = TempDir::new("bob-cli-projects-sync-checked-subproject");
    let vault = temp.path().join("vault");

    write_file(
        &vault.join("Parent.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[Child]]\n",
    );
    write_file(
        &vault.join("Child.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[Parent]]\n---\n- [x] #task Finish child #hide ^prj\n",
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("run bob projects sync");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("status: wip -> done")
            && out.contains(
                "removed #hide from ^prj  no non-hidden open tasks or open sub-projects"
            )
            && out.contains(
                "updated [[Child]] on ^prj  sub-project completed"
            )
            && out.contains(
                "2 projects - 1 status updated - 2 ^prj edited - 0 task schedules updated - 0 warnings"
            ),
        "unexpected sync output:\n{out}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("Parent.md")).expect("read parent"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent ^prj\n\t- 🧩 **Sub-projects:** ~~[[Child]]~~ ✅\n"
    );
    assert_eq!(
        fs::read_to_string(vault.join("Child.md")).expect("read child"),
        "---\ntype: [[project]]\nstatus: done\nparent: [[Parent]]\n---\n- [x] #task Finish child #hide ^prj\n"
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("rerun bob projects sync");

    assert_success(&output);
    assert!(
        stdout(&output).contains(
            "2 projects - 0 status updated - 0 ^prj edited - 0 task schedules updated - 0 warnings"
        ),
        "second run should have zero actions:\n{}",
        format_output(&output)
    );
}

#[test]
fn projects_sync_reopens_parent_ledger_when_child_prj_is_reopened_same_run() {
    let temp = TempDir::new("bob-cli-projects-sync-reopened-subproject");
    let vault = temp.path().join("vault");

    // The parent ledger shows the child as completed, but the child's
    // frontmatter is terminal while its ^prj task is open again. One sync run
    // should reopen the child to wip and flip the parent ledger back to open.
    write_file(
        &vault.join("Parent.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent #hide ^prj\n\t- 🧩 **Sub-projects:** ~~[[Child]]~~ ✅\n",
    );
    write_file(
        &vault.join("Child.md"),
        "---\ntype: [[project]]\nstatus: done\nparent: [[Parent]]\n---\n- [ ] #task Finish child #hide ^prj\n",
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("run bob projects sync");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("status: done -> wip  ^prj task opened")
            && out.contains(
                "removed #hide from ^prj  no non-hidden open tasks or open sub-projects"
            )
            && out.contains("updated [[Child]] on ^prj  open sub-project")
            && out.contains(
                "2 projects - 1 status updated - 2 ^prj edited - 0 task schedules updated - 0 warnings"
            ),
        "unexpected sync output:\n{out}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("Parent.md")).expect("read parent"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[Child]]\n"
    );
    assert_eq!(
        fs::read_to_string(vault.join("Child.md")).expect("read child"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[Parent]]\n---\n- [ ] #task Finish child ^prj\n"
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("rerun bob projects sync");

    assert_success(&output);
    assert!(
        stdout(&output).contains(
            "2 projects - 0 status updated - 0 ^prj edited - 0 task schedules updated - 0 warnings"
        ),
        "second run should have zero actions:\n{}",
        format_output(&output)
    );
}

#[test]
fn projects_sync_marks_canceled_subproject_same_run() {
    let temp = TempDir::new("bob-cli-projects-sync-canceled-subproject");
    let vault = temp.path().join("vault");

    write_file(
        &vault.join("Parent.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[Child]]\n",
    );
    write_file(
        &vault.join("Child.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[Parent]]\n---\n- [-] #task Stop child #hide ^prj\n",
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("run bob projects sync");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("status: wip -> canceled")
            && out.contains(
                "removed #hide from ^prj  no non-hidden open tasks or open sub-projects"
            )
            && out
                .contains("updated [[Child]] on ^prj  sub-project canceled")
            && out.contains(
                "2 projects - 1 status updated - 2 ^prj edited - 0 task schedules updated - 0 warnings"
            ),
        "unexpected sync output:\n{out}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("Parent.md")).expect("read parent"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent ^prj\n\t- 🧩 **Sub-projects:** ~~[[Child]]~~ ❌\n"
    );
    assert_eq!(
        fs::read_to_string(vault.join("Child.md")).expect("read child"),
        "---\ntype: [[project]]\nstatus: canceled\nparent: [[Parent]]\n---\n- [-] #task Stop child #hide ^prj\n"
    );
}

#[test]
fn projects_sync_orders_open_then_closed_subprojects_in_one_run() {
    let temp = TempDir::new("bob-cli-projects-sync-mixed-subprojects");
    let vault = temp.path().join("vault");

    write_file(
        &vault.join("Parent.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[DoneChild]] • [[CanceledChild]] • [[ExistingOpen]]\n",
    );
    write_file(
        &vault.join("ExistingOpen.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[Parent]]\n---\n- [ ] #task Finish existing #hide ^prj\n- [ ] #task Needs priority\n",
    );
    write_file(
        &vault.join("AddedOpen.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[Parent]]\n---\n- [ ] #task Finish added #hide ^prj\n- [ ] #task Needs priority\n",
    );
    write_file(
        &vault.join("DoneChild.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[Parent]]\n---\n- [x] #task Finish done #hide ^prj\n",
    );
    write_file(
        &vault.join("CanceledChild.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[Parent]]\n---\n- [-] #task Stop canceled #hide ^prj\n",
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("run bob projects sync");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("added [[AddedOpen]] to ^prj  open sub-project")
            && out.contains(
                "updated [[DoneChild]] on ^prj  sub-project completed"
            )
            && out.contains(
                "updated [[CanceledChild]] on ^prj  sub-project canceled"
            )
            && out.contains(
                "5 projects - 2 status updated - 3 ^prj edited - 0 task schedules updated - 0 warnings"
            ),
        "unexpected sync output:\n{out}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("Parent.md")).expect("read parent"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[AddedOpen]] • [[ExistingOpen]] • ~~[[CanceledChild]]~~ ❌ • ~~[[DoneChild]]~~ ✅\n"
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("rerun bob projects sync");

    assert_success(&output);
    assert!(
        stdout(&output).contains(
            "5 projects - 0 status updated - 0 ^prj edited - 0 task schedules updated - 0 warnings"
        ),
        "second run should have zero actions:\n{}",
        format_output(&output)
    );
}

#[test]
fn projects_sync_keeps_pruned_closed_entries_gone() {
    let temp = TempDir::new("bob-cli-projects-sync-curated-subprojects");
    let vault = temp.path().join("vault");

    write_file(
        &vault.join("Parent.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent ^prj\n\t- 🧩 **Sub-projects:** [[DeletedChild]] • [[ReparentedChild]] • ~~[[KeptDone]]~~ ✅\n",
    );
    write_file(
        &vault.join("KeptDone.md"),
        "---\ntype: [[project]]\nstatus: done\nparent: [[Parent]]\n---\n- [x] #task Finish kept #hide ^prj\n",
    );
    write_file(
        &vault.join("PrunedDone.md"),
        "---\ntype: [[project]]\nstatus: done\nparent: [[Parent]]\n---\n- [x] #task Finish pruned #hide ^prj\n",
    );
    write_file(
        &vault.join("ReparentedChild.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[OtherParent]]\n---\n- [ ] #task Finish reparented #hide ^prj\n- [ ] #task Needs priority\n",
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("run bob projects sync");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains(
            "removed [[DeletedChild]] from ^prj  no longer a sub-project"
        ) && out.contains(
            "removed [[ReparentedChild]] from ^prj  no longer a sub-project"
        ) && !out.contains("PrunedDone]]")
            && out.contains(
                "4 projects - 0 status updated - 2 ^prj edited - 0 task schedules updated - 0 warnings"
            ),
        "unexpected sync output:\n{out}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("Parent.md")).expect("read parent"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent ^prj\n\t- 🧩 **Sub-projects:** ~~[[KeptDone]]~~ ✅\n"
    );
}

#[test]
fn projects_sync_treats_children_without_open_prj_as_childless() {
    let temp = TempDir::new("bob-cli-projects-sync-no-open-subprojects");
    let vault = temp.path().join("vault");

    write_file(
        &vault.join("ParentMissingChild.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent #hide ^prj\n",
    );
    write_file(
        &vault.join("MissingPrjChild.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[ParentMissingChild]]\n---\n- [ ] #task Needs completion task\n",
    );
    write_file(
        &vault.join("ParentCheckedChild.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent #hide ^prj\n",
    );
    write_file(
        &vault.join("CheckedChild.md"),
        "---\ntype: [[project]]\nstatus: done\nparent: [[ParentCheckedChild]]\n---\n- [x] #task Finish child #hide ^prj\n",
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("run bob projects sync");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("active project has no ^prj task")
            && out.contains(
                "4 projects - 0 status updated - 2 ^prj edited - 0 task schedules updated - 1 warnings"
            ),
        "unexpected sync output:\n{out}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("ParentMissingChild.md")).expect("read parent"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent ^prj\n"
    );
    assert_eq!(
        fs::read_to_string(vault.join("ParentCheckedChild.md")).expect("read parent"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent ^prj\n"
    );
}

#[test]
fn projects_sync_preserves_user_sub_bullets_and_inserts_subprojects_line() {
    let temp = TempDir::new("bob-cli-projects-sync-user-sub-bullets");
    let vault = temp.path().join("vault");

    write_file(
        &vault.join("Parent.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent #hide ^prj\n\t- Remember to check notes\n\t- [[non_project_note]]\n",
    );
    write_file(
        &vault.join("Child.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[Parent]]\n---\n- [ ] #task Finish child #hide ^prj\n- [ ] #task Needs priority\n",
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("run bob projects sync");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("added [[Child]] to ^prj  open sub-project")
            && out.contains(
                "2 projects - 0 status updated - 1 ^prj edited - 0 task schedules updated - 0 warnings"
            ),
        "unexpected sync output:\n{out}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("Parent.md")).expect("read parent"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[Child]]\n\t- Remember to check notes\n\t- [[non_project_note]]\n"
    );
}

#[test]
fn projects_sync_normalizes_mangled_subprojects_line() {
    let temp = TempDir::new("bob-cli-projects-sync-subproject-line-update");
    let vault = temp.path().join("vault");

    write_file(
        &vault.join("Parent.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent #hide ^prj\n  - 🧩 **Sub-projects:** [[ChildBeta]] plus [[ChildAlpha]]\n",
    );
    write_file(
        &vault.join("ChildAlpha.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[Parent]]\n---\n- [ ] #task Finish alpha #hide ^prj\n- [ ] #task Needs priority\n",
    );
    write_file(
        &vault.join("ChildBeta.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[Parent]]\n---\n- [ ] #task Finish beta #hide ^prj\n- [ ] #task Needs priority\n",
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("run bob projects sync");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("updated sub-projects on ^prj  canonical format")
            && out.contains(
                "3 projects - 0 status updated - 1 ^prj edited - 0 task schedules updated - 0 warnings"
            ),
        "unexpected sync output:\n{out}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("Parent.md")).expect("read parent"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[ChildAlpha]] • [[ChildBeta]]\n"
    );
}

#[test]
fn projects_sync_subproject_line_dry_run_reports_without_writing() {
    let temp = TempDir::new("bob-cli-projects-sync-subproject-line-dry-run");
    let vault = temp.path().join("vault");

    write_file(
        &vault.join("ParentAdd.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish add #hide ^prj\n",
    );
    write_file(
        &vault.join("ChildAdd.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[ParentAdd]]\n---\n- [ ] #task Finish child #hide ^prj\n- [ ] #task Needs priority\n",
    );
    write_file(
        &vault.join("ParentRemove.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish remove #hide ^prj\n\t- 🧩 **Sub-projects:** [[OldChild]]\n- [ ] #task Needs priority\n",
    );
    write_file(
        &vault.join("ParentUpdate.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish update #hide ^prj\n\t- 🧩 **Sub-projects:** [[ChildUpdate]] plus notes\n",
    );
    write_file(
        &vault.join("ChildUpdate.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[ParentUpdate]]\n---\n- [ ] #task Finish update child #hide ^prj\n- [ ] #task Needs priority\n",
    );
    let snapshots = [
        (
            "ParentAdd.md",
            fs::read_to_string(vault.join("ParentAdd.md"))
                .expect("read parent add"),
        ),
        (
            "ParentRemove.md",
            fs::read_to_string(vault.join("ParentRemove.md"))
                .expect("read parent remove"),
        ),
        (
            "ParentUpdate.md",
            fs::read_to_string(vault.join("ParentUpdate.md"))
                .expect("read parent update"),
        ),
    ];

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--dry-run")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("run bob projects sync dry-run");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("would add [[ChildAdd]] to ^prj  open sub-project")
            && out.contains(
                "would remove [[OldChild]] from ^prj  no longer a sub-project"
            )
            && out.contains(
                "would update sub-projects on ^prj  canonical format"
            )
            && out.contains(
                "5 projects - 0 status updated - 3 ^prj edited - 0 task schedules updated - 0 warnings"
            ),
        "unexpected dry-run output:\n{out}"
    );
    for (name, snapshot) in snapshots {
        assert_eq!(
            fs::read_to_string(vault.join(name)).expect("read parent"),
            snapshot,
            "dry-run must not edit {name}"
        );
    }
}

#[test]
fn projects_sync_reports_prj_errors_without_aborting_scan() {
    let temp = TempDir::new("bob-cli-projects-sync-errors");
    let vault = temp.path().join("vault");

    write_file(
        &vault.join("Good.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [x] #task Good project #hide ^prj\n",
    );
    write_file(
        &vault.join("Malformed.md"),
        "---\ntype: [[project]]\n---\nComplete malformed project ^prj\n",
    );
    write_file(
        &vault.join("Multiple.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task One #hide ^prj\n- [ ] #task Two #hide ^prj\n",
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run bob projects sync with errors");

    assert_eq!(
        output.status.code(),
        Some(1),
        "project sync errors should exit 1:\n{}",
        format_output(&output)
    );
    assert!(
        stdout(&output)
            .contains("3 projects - 1 status updated - 0 ^prj edited - 0 task schedules updated - 0 warnings - 2 errors"),
        "unexpected sync summary:\n{}",
        format_output(&output)
    );
    let err = stderr(&output);
    assert!(
        err.contains("Malformed.md:4: malformed ^prj task")
            && err.contains("Multiple.md:5: multiple ^prj tasks"),
        "expected per-file project errors:\n{err}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("Good.md")).expect("read good"),
        "---\ntype: [[project]]\nstatus: done\n---\n- [x] #task Good project #hide ^prj\n"
    );
}

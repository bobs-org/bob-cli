//! Sync planning and subproject unit tests.
use super::*;

#[test]
fn project_sync_plan_flips_status_without_prj_edits_after_effective_status() {
    let contents = "---\ntype: [[project]]\nstatus: wip\n---\n- [x] #task Ship #hide ^prj\n";
    let mut issues = Vec::new();
    let project = parse_project(Path::new("Alpha.md"), contents, &mut issues)
        .expect("project");
    let plan = plan_project_sync(&project, false);

    assert!(issues.is_empty());
    assert_eq!(
        plan.changes,
        vec![ProjectChange::Status {
            from: "wip".to_string(),
            to: TargetProjectStatus::Done,
        }]
    );
    assert!(plan.warnings.is_empty());
}

#[test]
fn project_sync_plan_manages_prj_hide_tag_from_unhidden_count() {
    let mut issues = Vec::new();
    let stalled = parse_project(
        Path::new("Stalled.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n- [ ] #task Planned #hide\n",
        &mut issues,
    )
    .expect("project");
    assert_eq!(
        plan_project_sync(&stalled, false).changes,
        vec![ProjectChange::RemoveHideTag]
    );

    issues.clear();
    let has_unhidden = parse_project(
        Path::new("HasUnhidden.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task Ship ^prj\n- [ ] #task Needs surfacing\n",
        &mut issues,
    )
    .expect("project");
    assert_eq!(
        plan_project_sync(&has_unhidden, false).changes,
        vec![ProjectChange::AddHideTag {
            reason: AddHideReason::NonHiddenOpenTasks,
        }]
    );

    issues.clear();
    let all_hidden_helpers = parse_project(
        Path::new("AllHidden.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n- [ ] #task Hidden helper #hide\n",
        &mut issues,
    )
    .expect("project");
    assert_eq!(all_hidden_helpers.open_unhidden_count, 0);
    assert_eq!(
        plan_project_sync(&all_hidden_helpers, false).changes,
        vec![ProjectChange::RemoveHideTag]
    );

    assert!(issues.is_empty());
}

#[test]
fn project_sync_plan_manages_prj_hide_tag_from_open_subprojects() {
    let hidden_parent = parse_clean_project(
        "HiddenParent.md",
        "---\ntype: [[project]]\n---\n- [ ] #task Ship parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[Child]]\n",
    );
    assert!(
        plan_project_sync(&hidden_parent, true).changes.is_empty(),
        "existing #hide tag should be kept while open sub-projects exist"
    );

    let missing_hide_tag = parse_clean_project(
        "MissingHideTagParent.md",
        "---\ntype: [[project]]\n---\n- [ ] #task Ship parent ^prj\n\t- 🧩 **Sub-projects:** [[Child]]\n",
    );
    assert_eq!(
        plan_project_sync(&missing_hide_tag, true).changes,
        vec![ProjectChange::AddHideTag {
            reason: AddHideReason::OpenSubprojects,
        }]
    );

    let surfacing_parent = parse_clean_project(
        "SurfacingParent.md",
        "---\ntype: [[project]]\n---\n- [ ] #task Ship parent #hide ^prj\n",
    );
    assert_eq!(
        plan_project_sync(&surfacing_parent, false).changes,
        vec![ProjectChange::RemoveHideTag]
    );
}

#[test]
fn project_sync_plan_is_idempotent_when_prj_hide_tag_matches_dash_state() {
    let mut issues = Vec::new();
    let already_on_dash = parse_project(
        Path::new("AlreadyOnDash.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task Ship ^prj\n- [ ] #task Planned #hide\n",
        &mut issues,
    )
    .expect("project");
    assert!(plan_project_sync(&already_on_dash, false)
        .changes
        .is_empty());

    issues.clear();
    let already_hidden = parse_project(
        Path::new("AlreadyHidden.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n- [ ] #task Needs surfacing\n",
        &mut issues,
    )
    .expect("project");
    assert!(plan_project_sync(&already_hidden, false).changes.is_empty());
    assert!(issues.is_empty());
}

#[test]
fn project_sync_plan_keeps_inline_prj_scheduled_and_surfaces() {
    let mut issues = Vec::new();
    let project = parse_project(
        Path::new("Scheduled.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide [scheduled::2026-06-01] ^prj\n",
        &mut issues,
    )
    .expect("project");

    assert!(issues.is_empty());
    assert_eq!(
        plan_project_sync_at(
            &project,
            false,
            NaiveDate::from_ymd_opt(2026, 7, 10).unwrap(),
        )
        .changes,
        vec![ProjectChange::RemoveHideTag]
    );
}

#[test]
fn project_sync_plan_hoists_frontmatter_scheduled_onto_prj() {
    let project = parse_clean_project(
        "Hoist.md",
        "---\ntype: [[project]]\nscheduled: 2026-07-11\n---\n- [ ] #task Ship #hide [scheduled:: 2026-06-01] ^prj\n",
    );
    let today = NaiveDate::from_ymd_opt(2026, 7, 10).unwrap();
    let plan = plan_project_sync_at(&project, false, today);
    assert_eq!(
        plan.changes,
        vec![ProjectChange::HoistScheduled {
            scheduled: "2026-07-11".to_string(),
        }]
    );
}

#[test]
fn project_sync_plan_does_not_hoist_without_open_prj() {
    let project = parse_clean_project(
        "Missing.md",
        "---\ntype: [[project]]\nstatus: wip\nscheduled: 2026-07-11\n---\n- [ ] #task Needs completion task\n",
    );
    // The leftover frontmatter date must not flow into schedule
    // propagation, whether it is future or already past. Only a date
    // hoisted onto an open ^prj can become this run's project schedule.
    for today in [
        NaiveDate::from_ymd_opt(2026, 7, 10).unwrap(),
        NaiveDate::from_ymd_opt(2026, 7, 12).unwrap(),
    ] {
        let plan = plan_project_sync_at(&project, false, today);
        assert!(!plan.changes.iter().any(|change| matches!(
            change,
            ProjectChange::HoistScheduled { .. }
                | ProjectChange::ReconcileTaskSchedules { .. }
        )));
        assert!(plan.warnings.iter().any(|event| matches!(
            event,
            SyncEvent::Warning { message, .. }
                if message.contains("cannot hoist leftover scheduled")
        )));
    }
}

#[test]
fn project_sync_plan_leaves_terminal_frontmatter_scheduled_untouched() {
    let project = parse_clean_project(
        "Done.md",
        "---\ntype: [[project]]\nstatus: done\nscheduled: 2026-07-11\n---\n- [x] #task Ship #hide ^prj\n",
    );
    let plan = plan_project_sync_at(
        &project,
        false,
        NaiveDate::from_ymd_opt(2026, 7, 10).unwrap(),
    );
    assert!(!plan.changes.iter().any(|change| matches!(
        change,
        ProjectChange::HoistScheduled { .. }
            | ProjectChange::ReconcileTaskSchedules { .. }
    )));
}

#[test]
fn project_sync_plan_reopens_terminal_project_from_open_prj() {
    for terminal in ["done", "canceled"] {
        let contents = format!(
            "---\ntype: [[project]]\nstatus: {terminal}\n---\n- [ ] #task Ship #hide ^prj\n"
        );
        let project = parse_clean_project("Reopened.md", &contents);
        let plan = plan_project_sync(&project, false);

        // The open ^prj reopens the terminal project to wip, and because
        // the effective status is now active the surfacing pass runs in the
        // same sync, removing #hide from the otherwise-empty project.
        assert_eq!(
            plan.changes,
            vec![
                ProjectChange::Status {
                    from: terminal.to_string(),
                    to: TargetProjectStatus::Wip,
                },
                ProjectChange::RemoveHideTag,
            ],
            "status {terminal} should reopen to wip and surface"
        );
        assert!(plan.warnings.is_empty());
    }
}

#[test]
fn project_sync_plan_leaves_non_terminal_open_prj_status_untouched() {
    for status in ["wip", "waiting"] {
        let contents = format!(
            "---\ntype: [[project]]\nstatus: {status}\n---\n- [ ] #task Ship #hide ^prj\n"
        );
        let project = parse_clean_project("Active.md", &contents);
        let plan = plan_project_sync(&project, false);

        // Only terminal statuses are reopenable; an open ^prj never forces
        // waiting (or an already-active status) to wip.
        assert!(
            !plan
                .changes
                .iter()
                .any(|change| matches!(change, ProjectChange::Status { .. })),
            "status {status} should not be rewritten by an open ^prj"
        );
    }
}

#[test]
fn project_sync_plan_warns_on_placeholder_while_reopening() {
    let contents = format!(
        "---\ntype: [[project]]\nstatus: done\n---\n- [ ] #task {PLACEHOLDER_CRITERIA} #hide ^prj\n"
    );
    let mut issues = Vec::new();
    let project = parse_project(Path::new("Alpha.md"), &contents, &mut issues)
        .expect("project");
    let plan = plan_project_sync(&project, false);

    assert!(issues.is_empty());
    // The drift between terminal frontmatter and an open ^prj is now an
    // explicit reopen rather than a warning, but the placeholder warning is
    // still surfaced.
    assert!(plan.changes.iter().any(|change| matches!(
        change,
        ProjectChange::Status {
            to: TargetProjectStatus::Wip,
            ..
        }
    )));
    assert!(
        !plan.warnings
            .iter()
            .any(|event| matches!(event, SyncEvent::Warning { message, .. } if message.contains("still open")))
    );
    assert_eq!(plan.warnings.len(), 1);
    assert!(
        plan.warnings
            .iter()
            .any(|event| matches!(event, SyncEvent::Warning { message, .. } if message.contains("placeholder")))
    );
}

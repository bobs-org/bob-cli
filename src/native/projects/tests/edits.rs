//! Edit, schedule, and frontmatter unit tests.
use super::*;

#[test]
fn project_changes_replace_status_append_missing_status_and_add_hide_tag() {
    let contents =
        "---\ntype: [[project]]\nstatus: waiting\n---\n- [ ] #task Ship ^prj\n";
    let output = apply_changes(
        contents,
        &[
            ProjectChange::Status {
                from: "waiting".to_string(),
                to: TargetProjectStatus::Canceled,
            },
            ProjectChange::AddHideTag {
                reason: AddHideReason::NonHiddenOpenTasks,
            },
        ],
    );
    assert_eq!(
        output,
        "---\ntype: [[project]]\nstatus: canceled\n---\n- [ ] #task Ship #hide ^prj\n"
    );

    let output = apply_changes(
        "---\ntype: [[project]]\n---\n- [x] #task Ship #hide ^prj\n",
        &[ProjectChange::Status {
            from: "wip".to_string(),
            to: TargetProjectStatus::Done,
        }],
    );
    assert_eq!(
        output,
        "---\ntype: [[project]]\nstatus: done\n---\n- [x] #task Ship #hide ^prj\n"
    );
}

#[test]
fn project_changes_remove_prj_fields_with_adjacent_whitespace() {
    let output = apply_changes(
        "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide [scheduled::2026-06-01] ^prj\n",
        &[
            ProjectChange::RemoveHideTag,
            ProjectChange::RemoveScheduled {
                scheduled: "2026-06-01".to_string(),
            },
        ],
    );
    assert_eq!(
        output,
        "---\ntype: [[project]]\n---\n- [ ] #task Ship ^prj\n"
    );
}

#[test]
fn project_changes_remove_prj_hide_tag_with_crlf() {
    let output = apply_changes(
        "---\r\ntype: [[project]]\r\n---\r\n- [ ] #task Ship #hide ^prj\r\n",
        &[ProjectChange::RemoveHideTag],
    );
    assert_eq!(
        output,
        "---\r\ntype: [[project]]\r\n---\r\n- [ ] #task Ship ^prj\r\n"
    );
}

#[test]
fn project_changes_preserve_crlf_when_appending_status() {
    let output = apply_changes(
        "---\r\ntype: [[project]]\r\n---\r\n- [x] #task Ship #hide ^prj\r\n",
        &[ProjectChange::Status {
            from: "wip".to_string(),
            to: TargetProjectStatus::Done,
        }],
    );
    assert_eq!(
        output,
        "---\r\ntype: [[project]]\r\nstatus: done\r\n---\r\n- [x] #task Ship #hide ^prj\r\n"
    );
}

#[test]
fn project_changes_insert_subproject_links_after_prj_with_tab_indent() {
    let desired = [open_subproject("Child")];
    let output = apply_subproject_changes(
        "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n## Tasks\n",
        &[ProjectChange::AddSubprojectLink {
            stem: "Child".to_string(),
            state: SubprojectState::Open,
        }],
        &desired,
    );

    assert_eq!(
        output,
        "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n\t- 🧩 **Sub-projects:** [[Child]]\n## Tasks\n"
    );
}

#[test]
fn project_changes_insert_subproject_line_above_user_bullets() {
    let desired = [open_subproject("Beta")];
    let output = apply_subproject_changes(
        "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n  - [[Alpha]]\n  - prose notes\n",
        &[ProjectChange::AddSubprojectLink {
            stem: "Beta".to_string(),
            state: SubprojectState::Open,
        }],
        &desired,
    );

    assert_eq!(
        output,
        "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n\t- 🧩 **Sub-projects:** [[Beta]]\n  - [[Alpha]]\n  - prose notes\n"
    );
}

#[test]
fn project_changes_rewrite_subproject_line_in_place() {
    let desired = [open_subproject("Alpha"), open_subproject("Beta")];
    let output = apply_subproject_changes(
        "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n\t- user notes\n  - 🧩 **Sub-projects:** [[Alpha]]\n",
        &[ProjectChange::AddSubprojectLink {
            stem: "Beta".to_string(),
            state: SubprojectState::Open,
        }],
        &desired,
    );

    assert_eq!(
        output,
        "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n\t- user notes\n\t- 🧩 **Sub-projects:** [[Alpha]] • [[Beta]]\n"
    );
}

#[test]
fn project_changes_mark_last_child_closed_and_keep_subproject_line() {
    let desired = [done_subproject("OldChild")];
    let output = apply_subproject_changes(
        "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n\t- 🧩 **Sub-projects:** [[OldChild]]\n\t- user notes\n",
        &[ProjectChange::MarkSubproject {
            stem: "OldChild".to_string(),
            state: SubprojectState::Done,
        }],
        &desired,
    );

    assert_eq!(
        output,
        "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n\t- 🧩 **Sub-projects:** ~~[[OldChild]]~~ ✅\n\t- user notes\n"
    );
}

#[test]
fn project_changes_delete_subproject_line_for_stale_child() {
    let output = apply_subproject_changes(
        "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n\t- 🧩 **Sub-projects:** [[OldChild]]\n\t- user notes\n",
        &[ProjectChange::RemoveSubprojectLink {
            stem: "OldChild".to_string(),
        }],
        &[],
    );

    assert_eq!(
        output,
        "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n\t- user notes\n"
    );
}

#[test]
fn project_changes_clean_duplicate_subproject_marker_lines() {
    let desired = [open_subproject("Alpha"), open_subproject("Beta")];
    let output = apply_subproject_changes(
        "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n\t- 🧩 **Sub-projects:** [[Beta]] • [[Alpha]] extra\n\t- keep me\n\t- 🧩 **Sub-projects:** [[Alpha]]\n",
        &[ProjectChange::NormalizeSubprojects],
        &desired,
    );

    assert_eq!(
        output,
        "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n\t- 🧩 **Sub-projects:** [[Alpha]] • [[Beta]]\n\t- keep me\n"
    );
}

#[test]
fn project_changes_preserve_crlf_for_subproject_link_insertions() {
    let desired = [open_subproject("Child")];
    let output = apply_subproject_changes(
        "---\r\ntype: [[project]]\r\n---\r\n- [ ] #task Ship #hide ^prj\r\n## Tasks\r\n",
        &[ProjectChange::AddSubprojectLink {
            stem: "Child".to_string(),
            state: SubprojectState::Open,
        }],
        &desired,
    );

    assert_eq!(
        output,
        "---\r\ntype: [[project]]\r\n---\r\n- [ ] #task Ship #hide ^prj\r\n\t- 🧩 **Sub-projects:** [[Child]]\r\n## Tasks\r\n"
    );
}

#[test]
fn project_changes_insert_subproject_links_after_final_prj_line() {
    let desired = [open_subproject("Child")];
    let output = apply_subproject_changes(
        "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj",
        &[ProjectChange::AddSubprojectLink {
            stem: "Child".to_string(),
            state: SubprojectState::Open,
        }],
        &desired,
    );

    assert_eq!(
        output,
        "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n\t- 🧩 **Sub-projects:** [[Child]]"
    );
}

#[test]
fn project_schedule_accepts_quoted_dates_and_rejects_bad_dates() {
    for scalar in ["2026-07-11", "\"2026-07-11\"", "'2026-07-11'"] {
        let contents = format!(
            "---\ntype: [[project]]\nscheduled: {scalar}\n---\n- [ ] #task Ship ^prj\n"
        );
        let project = parse_clean_project("Valid.md", &contents);
        assert_eq!(
            project.scheduled,
            Some(ProjectSchedule {
                raw: "2026-07-11".to_string(),
                date: NaiveDate::from_ymd_opt(2026, 7, 11).unwrap(),
            })
        );
    }

    for (scalar, expected) in [
        ("", "YYYY-MM-DD"),
        ("2026-7-11", "YYYY-MM-DD"),
        ("2026-02-30", "not a valid calendar date"),
    ] {
        let contents = format!(
            "---\ntype: [[project]]\nscheduled: {scalar}\n---\n- [ ] #task Ship ^prj\n"
        );
        let mut issues = Vec::new();
        let project =
            parse_project(Path::new("Invalid.md"), &contents, &mut issues)
                .expect("project");
        assert!(project.scheduled.is_none());
        assert_eq!(issues[0].line_number, Some(3));
        assert!(issues[0].message.contains(expected), "{issues:?}");
    }
}

#[test]
fn frontmatter_keys_must_start_at_column_zero() {
    let mut issues = Vec::new();
    let project = parse_project(
        Path::new("Indented.md"),
        "---\ntype: [[project]]\nnotes: |\n  scheduled: nope\n---\n- [ ] #task Ship ^prj\n",
        &mut issues,
    )
    .expect("project");
    assert!(issues.is_empty());
    assert!(project.scheduled.is_none());
    let frontmatter =
        parse_frontmatter("---\nnotes: |\n  status: done\n---\n").unwrap();
    assert_eq!(frontmatter_value(&frontmatter, "status"), None);
}

#[test]
fn terminal_projects_do_not_reconcile_task_schedules() {
    let project = parse_clean_project(
        "Done.md",
        "---\ntype: [[project]]\nstatus: done\nscheduled: 2026-07-10\n---\n- [x] #task Finished #hide ^prj\n",
    );
    let plan = plan_project_sync_at(
        &project,
        &[],
        NaiveDate::from_ymd_opt(2026, 7, 11).unwrap(),
    );
    assert!(!plan.changes.iter().any(|change| matches!(
        change,
        ProjectChange::ReconcileTaskSchedules { .. }
    )));
}

#[test]
fn schedule_only_issues_still_allow_subproject_aggregation() {
    assert!(issues_allow_subproject(&[ScanIssue::line(
        "Child.md",
        4,
        "scheduled must be a calendar date in YYYY-MM-DD format",
    )]));
    assert!(!issues_allow_subproject(&[ScanIssue::line(
        "Child.md",
        5,
        "multiple ^prj tasks found",
    )]));
}

#[test]
fn scheduled_tasks_precede_prj_surfacing_at_local_date_boundary() {
    let future = parse_clean_project(
        "Future.md",
        "---\ntype: [[project]]\nscheduled: 2026-07-11\n---\n- [ ] #task Ship ^prj\n- [ ] #task Work\n",
    );
    let today = NaiveDate::from_ymd_opt(2026, 7, 10).unwrap();
    assert_eq!(
        plan_project_sync_at(&future, &[], today).changes,
        vec![ProjectChange::ReconcileTaskSchedules {
            scheduled: "2026-07-11".to_string(),
            policy: TaskSchedulePolicy::for_schedule(
                NaiveDate::from_ymd_opt(2026, 7, 11).unwrap(),
                today,
                2,
            ),
            scheduled_task_count: 1,
            removed_hide_count: 0,
            prj_hide_changed: true,
        }]
    );

    for due_today in [
        NaiveDate::from_ymd_opt(2026, 7, 11).unwrap(),
        NaiveDate::from_ymd_opt(2026, 7, 12).unwrap(),
    ] {
        let due = parse_clean_project(
            "Due.md",
            "---\ntype: [[project]]\nscheduled: 2026-07-11\n---\n- [ ] #task Ship #hide ^prj\n- [x] #task Done #hide\n",
        );
        assert_eq!(
            plan_project_sync_at(&due, &[], due_today).changes,
            vec![ProjectChange::ReconcileTaskSchedules {
                scheduled: "2026-07-11".to_string(),
                policy: TaskSchedulePolicy::for_schedule(
                    NaiveDate::from_ymd_opt(2026, 7, 11).unwrap(),
                    due_today,
                    2,
                ),
                scheduled_task_count: 0,
                removed_hide_count: 1,
                prj_hide_changed: false,
            }]
        );
    }

    let sole_prj = parse_clean_project(
        "Sole.md",
        "---\ntype: [[project]]\nscheduled: 2026-07-11\n---\n- [ ] #task Ship #hide ^prj\n",
    );
    assert_eq!(
        plan_project_sync_at(
            &sole_prj,
            &[],
            NaiveDate::from_ymd_opt(2026, 7, 11).unwrap(),
        )
        .changes,
        vec![ProjectChange::ReconcileTaskSchedules {
            scheduled: "2026-07-11".to_string(),
            policy: TaskSchedulePolicy::for_schedule(
                NaiveDate::from_ymd_opt(2026, 7, 11).unwrap(),
                NaiveDate::from_ymd_opt(2026, 7, 11).unwrap(),
                1,
            ),
            scheduled_task_count: 0,
            removed_hide_count: 0,
            prj_hide_changed: true,
        }]
    );
}

#[test]
fn scheduled_tasks_keep_subproject_ledger_planning() {
    let project = parse_clean_project(
        "Parent.md",
        "---\ntype: [[project]]\nscheduled: 2026-07-11\n---\n- [ ] #task Ship ^prj\n",
    );
    let plan = plan_project_sync_at(
        &project,
        &[open_subproject("Child")],
        NaiveDate::from_ymd_opt(2026, 7, 10).unwrap(),
    );

    assert!(plan.changes.iter().any(|change| matches!(
        change,
        ProjectChange::ReconcileTaskSchedules {
            policy: TaskSchedulePolicy { future: true, .. },
            prj_hide_changed: true,
            ..
        }
    )));
    assert!(plan.changes.iter().any(|change| matches!(
        change,
        ProjectChange::AddSubprojectLink { stem, .. } if stem == "Child"
    )));
    assert_eq!(
        plan.desired_subprojects,
        Some(vec![open_subproject("Child")])
    );
}

#[test]
fn task_schedule_edits_cover_contract_and_preserve_markdown() {
    let contents = "---\r\ntype: [[project]]\r\nscheduled: 2026-07-11\r\n---\r\n- [ ] #task Ship [p:: 1] [scheduled:: stale] ^prj\r\n  1. [ ] Nested missing #hide ^nested\r\n> - [*] Equal (scheduled:: 2026-07-11) #hide\r\n- [/] Later [scheduled:: 2026-07-12] #hideaway #hide\r\n- [?] Earlier [scheduled:: 2026-07-10] #hide #hide\r\n- [x] Done #hide\r\n- [X] Done upper #hide\r\n- [-] Canceled #hidden #hide\r\n- [!] Custom #hide\r\n- [ ] Duplicate [scheduled:: 2026-07-09] (scheduled:: 2026-07-12) #hide\r\n```md\r\n- [ ] fenced example #hide\r\n```\r\nThis mentions - [ ] checkbox prose\r\n";
    let future = apply_changes(
        contents,
        &[
            ProjectChange::ReconcileTaskSchedules {
                scheduled: "2026-07-11".to_string(),
                policy: TaskSchedulePolicy::for_schedule(
                    NaiveDate::from_ymd_opt(2026, 7, 11).unwrap(),
                    NaiveDate::from_ymd_opt(2026, 7, 10).unwrap(),
                    10,
                ),
                scheduled_task_count: 2,
                removed_hide_count: 8,
                prj_hide_changed: true,
            },
            ProjectChange::RemoveScheduled {
                scheduled: "stale".to_string(),
            },
        ],
    );
    assert_eq!(
        future,
        "---\r\ntype: [[project]]\r\nscheduled: 2026-07-11\r\n---\r\n- [ ] #task Ship [p:: 1] #hide ^prj\r\n  1. [ ] Nested missing [scheduled:: 2026-07-11] ^nested\r\n> - [*] Equal (scheduled:: 2026-07-11)\r\n- [/] Later [scheduled:: 2026-07-12] #hideaway\r\n- [?] Earlier [scheduled:: 2026-07-11]\r\n- [x] Done\r\n- [X] Done upper\r\n- [-] Canceled #hidden\r\n- [!] Custom\r\n- [ ] Duplicate [scheduled:: 2026-07-09] (scheduled:: 2026-07-12) #hide\r\n```md\r\n- [ ] fenced example #hide\r\n```\r\nThis mentions - [ ] checkbox prose\r\n"
    );

    assert_eq!(
        apply_changes(
            &future,
            &[ProjectChange::ReconcileTaskSchedules {
                scheduled: "2026-07-11".to_string(),
                policy: TaskSchedulePolicy::for_schedule(
                    NaiveDate::from_ymd_opt(2026, 7, 11).unwrap(),
                    NaiveDate::from_ymd_opt(2026, 7, 11).unwrap(),
                    10,
                ),
                scheduled_task_count: 0,
                removed_hide_count: 0,
                prj_hide_changed: false,
            }],
        ),
        future
    );

    let sole_prj = "---\ntype: [[project]]\nscheduled: 2026-07-11\n---\n- [ ] #task Ship #hide ^prj\n";
    assert_eq!(
        apply_changes(
            sole_prj,
            &[ProjectChange::ReconcileTaskSchedules {
                scheduled: "2026-07-11".to_string(),
                policy: TaskSchedulePolicy::for_schedule(
                    NaiveDate::from_ymd_opt(2026, 7, 11).unwrap(),
                    NaiveDate::from_ymd_opt(2026, 7, 11).unwrap(),
                    1,
                ),
                scheduled_task_count: 0,
                removed_hide_count: 0,
                prj_hide_changed: true,
            }],
        ),
        "---\ntype: [[project]]\nscheduled: 2026-07-11\n---\n- [ ] #task Ship ^prj\n"
    );
}

#[test]
fn project_parser_splits_surfacing_and_dash_visibility_counts() {
    let project = parse_clean_project(
        "Counts.md",
        "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n- [ ] #task Ready\n- [?] #task Dependency blocked\n- [ ] #task Future [scheduled:: 2999-01-01]\n- [ ] #task Parenthesized future (scheduled:: 2999-01-02)\n- [ ] #task Hidden #hide\n",
    );
    assert_eq!(project.open_unhidden_count, 4);
    assert_eq!(project.dash_visible_count, 1);
}

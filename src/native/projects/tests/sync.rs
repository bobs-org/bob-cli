//! Sync planning and subproject unit tests.
use super::*;

#[test]
fn project_sync_plan_flips_status_without_prj_edits_after_effective_status() {
    let contents = "---\ntype: [[project]]\nstatus: wip\n---\n- [x] #task Ship #hide ^prj\n";
    let mut issues = Vec::new();
    let project = parse_project(Path::new("Alpha.md"), contents, &mut issues)
        .expect("project");
    let plan = plan_project_sync(&project, &[]);

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
        plan_project_sync(&stalled, &[]).changes,
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
        plan_project_sync(&has_unhidden, &[]).changes,
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
        plan_project_sync(&all_hidden_helpers, &[]).changes,
        vec![ProjectChange::RemoveHideTag]
    );

    assert!(issues.is_empty());
}

#[test]
fn project_sync_plan_manages_prj_hide_tag_from_open_subprojects() {
    let children = [open_subproject("Child")];
    let hidden_parent = parse_clean_project(
        "HiddenParent.md",
        "---\ntype: [[project]]\n---\n- [ ] #task Ship parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[Child]]\n",
    );
    assert!(
        plan_project_sync(&hidden_parent, &children)
            .changes
            .is_empty(),
        "existing #hide tag should be kept while open sub-projects exist"
    );

    let missing_hide_tag = parse_clean_project(
        "MissingHideTagParent.md",
        "---\ntype: [[project]]\n---\n- [ ] #task Ship parent ^prj\n\t- 🧩 **Sub-projects:** [[Child]]\n",
    );
    assert_eq!(
        plan_project_sync(&missing_hide_tag, &children).changes,
        vec![ProjectChange::AddHideTag {
            reason: AddHideReason::OpenSubprojects,
        }]
    );

    let surfacing_parent = parse_clean_project(
        "SurfacingParent.md",
        "---\ntype: [[project]]\n---\n- [ ] #task Ship parent #hide ^prj\n",
    );
    assert_eq!(
        plan_project_sync(&surfacing_parent, &[]).changes,
        vec![ProjectChange::RemoveHideTag]
    );
}

#[test]
fn project_sync_plan_reconciles_subprojects_marker_line() {
    let project = parse_clean_project(
        "Parent.md",
        "---\ntype: [[project]]\n---\n- [ ] #task Ship parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[ExistingChild]] • [[stale_child]]\n\t- [[MentionedChild]] kickoff notes\n\t- prose with [[ManualOnly]] link\n",
    );
    let children = [
        open_subproject("AnotherChild"),
        open_subproject("ExistingChild"),
        open_subproject("MentionedChild"),
    ];

    assert_eq!(
        plan_project_sync(&project, &children).changes,
        vec![
            ProjectChange::AddSubprojectLink {
                stem: "AnotherChild".to_string(),
                state: SubprojectState::Open,
            },
            ProjectChange::AddSubprojectLink {
                stem: "MentionedChild".to_string(),
                state: SubprojectState::Open,
            },
            ProjectChange::RemoveSubprojectLink {
                stem: "stale_child".to_string(),
            },
        ]
    );
}

#[test]
fn project_sync_plan_matches_subproject_links_case_insensitively() {
    let project = parse_clean_project(
        "Parent.md",
        "---\ntype: [[project]]\n---\n- [ ] #task Ship parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[Child]]\n",
    );
    let children = [open_subproject("child")];

    let changes = plan_project_sync(&project, &children).changes;
    assert!(!changes.iter().any(|change| matches!(
        change,
        ProjectChange::AddSubprojectLink { .. }
            | ProjectChange::RemoveSubprojectLink { .. }
    )));
    assert_eq!(changes, vec![ProjectChange::NormalizeSubprojects]);
}

#[test]
fn project_sync_plan_normalizes_subprojects_marker_drift() {
    let children = [open_subproject("Alpha"), open_subproject("Beta")];
    for contents in [
        "---\ntype: [[project]]\n---\n- [ ] #task Ship parent #hide ^prj\n  - 🧩 **Sub-projects:** [[Beta]], [[Alpha]]\n",
        "---\ntype: [[project]]\n---\n- [ ] #task Ship parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[Alpha]] • [[Beta]]\n\t- 🧩 **Sub-projects:** [[Alpha]] • [[Beta]]\n",
    ] {
        let project = parse_clean_project("Parent.md", contents);
        assert_eq!(
            plan_project_sync(&project, &children).changes,
            vec![ProjectChange::NormalizeSubprojects]
        );
    }
}

#[test]
fn project_sync_plan_marks_tracked_closed_subprojects() {
    let project = parse_clean_project(
        "Parent.md",
        "---\ntype: [[project]]\n---\n- [ ] #task Ship parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[DoneChild]] • ~~[[CanceledChild]]~~ ❌ • [[OpenChild]]\n",
    );
    let children = [
        done_subproject("DoneChild"),
        canceled_subproject("CanceledChild"),
        open_subproject("OpenChild"),
        done_subproject("PrunedDoneChild"),
    ];

    let plan = plan_project_sync(&project, &children);
    assert_eq!(
        plan.changes,
        vec![ProjectChange::MarkSubproject {
            stem: "DoneChild".to_string(),
            state: SubprojectState::Done,
        }]
    );
    assert_eq!(
        plan.desired_subprojects,
        Some(vec![
            open_subproject("OpenChild"),
            canceled_subproject("CanceledChild"),
            done_subproject("DoneChild"),
        ])
    );
}

#[test]
fn project_sync_plan_keeps_canonical_closed_subprojects_idempotent() {
    let project = parse_clean_project(
        "Parent.md",
        "---\ntype: [[project]]\n---\n- [ ] #task Ship parent ^prj\n\t- 🧩 **Sub-projects:** ~~[[CanceledChild]]~~ ❌ • ~~[[DoneChild]]~~ ✅\n",
    );
    let children = [
        done_subproject("DoneChild"),
        canceled_subproject("CanceledChild"),
    ];

    assert!(plan_project_sync(&project, &children).changes.is_empty());
}

#[test]
fn render_subprojects_line_formats_closed_children_after_open_children() {
    let entries = [
        future_subproject("Gamma", SubprojectState::Done),
        open_subproject("Beta"),
        canceled_subproject("Delta"),
        future_subproject("Alpha", SubprojectState::Open),
    ];

    assert_eq!(
        render_subprojects_line_text(&entries),
        "- 🧩 **Sub-projects:** 🗓️ [[Alpha]] • [[Beta]] • ~~[[Delta]]~~ ❌ • 🗓️ ~~[[Gamma]]~~ ✅"
    );
}

#[test]
fn subproject_display_parser_scopes_schedule_and_lifecycle_markers() {
    let displays = subproject_displays_in_marker_line(
        "- 🧩 **Sub-projects:** 🗓️ [[FutureOpen]] • [[PlainOpen]] • 🗓️ ~~[[FutureDone]]~~ ✅ • ~~[[PlainCanceled]]~~ ❌",
    );

    assert_eq!(
        displays.get("futureopen"),
        Some(&SubprojectDisplay {
            state: SubprojectState::Open,
            future_scheduled: true,
        })
    );
    assert_eq!(
        displays.get("plainopen"),
        Some(&SubprojectDisplay {
            state: SubprojectState::Open,
            future_scheduled: false,
        })
    );
    assert_eq!(
        displays.get("futuredone"),
        Some(&SubprojectDisplay {
            state: SubprojectState::Done,
            future_scheduled: true,
        })
    );
    assert_eq!(
        displays.get("plaincanceled"),
        Some(&SubprojectDisplay {
            state: SubprojectState::Canceled,
            future_scheduled: false,
        })
    );
}

#[test]
fn project_sync_plan_reconciles_subproject_schedule_markers() {
    let project = parse_clean_project(
        "Parent.md",
        "---\ntype: [[project]]\n---\n- [ ] #task Ship parent #hide ^prj\n\t- 🧩 **Sub-projects:** 🗓️ [[DueOpen]] • [[FutureOpen]] • 🗓️ ~~[[DueClosed]]~~ ❌ • ~~[[FutureClosed]]~~ ✅\n",
    );
    let children = [
        open_subproject("DueOpen"),
        future_subproject("FutureOpen", SubprojectState::Open),
        canceled_subproject("DueClosed"),
        future_subproject("FutureClosed", SubprojectState::Done),
    ];

    let plan = plan_project_sync(&project, &children);
    assert_eq!(
        plan.changes,
        vec![
            ProjectChange::RemoveSubprojectScheduleMarker {
                stem: "DueOpen".to_string(),
            },
            ProjectChange::AddSubprojectScheduleMarker {
                stem: "FutureOpen".to_string(),
            },
            ProjectChange::RemoveSubprojectScheduleMarker {
                stem: "DueClosed".to_string(),
            },
            ProjectChange::AddSubprojectScheduleMarker {
                stem: "FutureClosed".to_string(),
            },
        ]
    );
    let desired = plan.desired_subprojects.expect("desired ledger");
    let output = apply_subproject_changes(
        "---\ntype: [[project]]\n---\n- [ ] #task Ship parent #hide ^prj\n\t- 🧩 **Sub-projects:** 🗓️ [[DueOpen]] • [[FutureOpen]] • 🗓️ ~~[[DueClosed]]~~ ❌ • ~~[[FutureClosed]]~~ ✅\n",
        &plan.changes,
        &desired,
    );
    assert_eq!(
        output,
        "---\ntype: [[project]]\n---\n- [ ] #task Ship parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[DueOpen]] • 🗓️ [[FutureOpen]] • ~~[[DueClosed]]~~ ❌ • 🗓️ ~~[[FutureClosed]]~~ ✅\n"
    );

    let canonical = parse_clean_project("Parent.md", &output);
    assert!(plan_project_sync(&canonical, &children).changes.is_empty());
}

#[test]
fn project_sync_plan_treats_user_sub_bullets_as_user_owned() {
    let project = parse_clean_project(
        "Parent.md",
        "---\ntype: [[project]]\n---\n- [ ] #task Ship parent #hide ^prj\n\t- [[Child]]\n\t- [[ManualOnly]] kickoff notes\n- [ ] #task Needs priority\n",
    );

    assert_eq!(
        plan_project_sync(&project, &[open_subproject("Child")]).changes,
        vec![ProjectChange::AddSubprojectLink {
            stem: "Child".to_string(),
            state: SubprojectState::Open,
        }]
    );
}

#[test]
fn project_sync_plan_skips_subproject_links_without_open_prj_edits() {
    let missing = parse_clean_project(
        "Missing.md",
        "---\ntype: [[project]]\n---\n- [ ] #task Needs completion task\n",
    );
    let checked = parse_clean_project(
        "Checked.md",
        "---\ntype: [[project]]\n---\n- [x] #task Ship checked #hide ^prj\n\t- [[OldChild]]\n",
    );
    let terminal = parse_clean_project(
        "Terminal.md",
        "---\ntype: [[project]]\nstatus: canceled\n---\n- [-] #task Ship terminal #hide ^prj\n\t- [[OldChild]]\n",
    );
    let children = [open_subproject("Child")];

    for project in [&missing, &checked, &terminal] {
        assert!(!plan_project_sync(project, &children).changes.iter().any(
            |change| matches!(
                change,
                ProjectChange::AddSubprojectLink { .. }
                    | ProjectChange::RemoveSubprojectLink { .. }
                    | ProjectChange::MarkSubproject { .. }
                    | ProjectChange::NormalizeSubprojects
            )
        ));
    }
}

#[test]
fn subproject_parent_links_classify_open_and_terminal_prj_children() {
    let parent = parse_clean_project(
        "Projects/Parent.md",
        "---\ntype: [[project]]\n---\n- [ ] #task Ship parent #hide ^prj\n",
    );
    let open_child = parse_clean_project(
        "Projects/OpenChild.md",
        "---\ntype: [[project]]\nparent: [[Projects/Parent]]\n---\n- [ ] #task Ship child #hide ^prj\n",
    );
    let path_case_child = parse_clean_project(
        "Projects/PathCaseChild.md",
        "---\ntype: [[project]]\nparent: [[areas/PARENT#Now|Parent]]\n---\n- [ ] #task Ship child #hide ^prj\n",
    );
    let terminal_status_open_child = parse_clean_project(
        "Projects/TerminalStatusOpenChild.md",
        "---\ntype: [[project]]\nstatus: done\nparent: [[Parent]]\n---\n- [ ] #task Ship child #hide ^prj\n",
    );
    let checked_child = parse_clean_project(
        "Projects/CheckedChild.md",
        "---\ntype: [[project]]\nparent: [[Parent]]\n---\n- [x] #task Ship child #hide ^prj\n",
    );
    let canceled_child = parse_clean_project(
        "Projects/CanceledChild.md",
        "---\ntype: [[project]]\nparent: [[Parent]]\n---\n- [-] #task Ship child #hide ^prj\n",
    );
    let missing_prj_child = parse_clean_project(
        "Projects/MissingPrjChild.md",
        "---\ntype: [[project]]\nparent: [[Parent]]\n---\n- [ ] #task Needs completion task\n",
    );
    let self_link = parse_clean_project(
        "Projects/Self.md",
        "---\ntype: [[project]]\nparent: [[Self]]\n---\n- [ ] #task Ship self #hide ^prj\n",
    );
    let area_child = parse_clean_project(
        "Projects/AreaChild.md",
        "---\ntype: [[project]]\nparent: [[Area]]\n---\n- [ ] #task Ship child #hide ^prj\n",
    );

    let mut issues = Vec::new();
    let malformed_child = parse_project(
        Path::new("Projects/MalformedChild.md"),
        "---\ntype: [[project]]\nparent: [[Parent]]\n---\nShip malformed child ^prj\n",
        &mut issues,
    )
    .expect("malformed project");
    let multiple_child = parse_project(
        Path::new("Projects/MultipleChild.md"),
        "---\ntype: [[project]]\nparent: [[Parent]]\n---\n- [ ] #task One #hide ^prj\n- [ ] #task Two #hide ^prj\n",
        &mut issues,
    )
    .expect("multiple project");
    assert_eq!(issues.len(), 2);

    let today = NaiveDate::from_ymd_opt(2026, 7, 10).unwrap();
    let children_by_parent = subproject_children_by_parent_link_name(
        [
            &open_child,
            &path_case_child,
            &terminal_status_open_child,
            &checked_child,
            &canceled_child,
            &missing_prj_child,
            &malformed_child,
            &multiple_child,
            &self_link,
            &area_child,
        ],
        today,
    );

    assert_eq!(
        children_by_parent.get(&parent.link_name),
        Some(&vec![
            canceled_subproject("CanceledChild"),
            done_subproject("CheckedChild"),
            open_subproject("OpenChild"),
            open_subproject("PathCaseChild"),
            open_subproject("TerminalStatusOpenChild"),
        ])
    );
    assert_eq!(
        children_by_parent.get("area"),
        Some(&vec![open_subproject("AreaChild")])
    );
    assert!(!children_by_parent.contains_key(&self_link.link_name));

    let non_parent_links =
        subproject_children_by_parent_link_name([&area_child], today);
    assert!(!non_parent_links.contains_key(&parent.link_name));
}

#[test]
fn subproject_state_treats_terminal_open_prj_child_as_open() {
    // A child whose frontmatter is terminal but whose ^prj task is open
    // again should count as open for parent ledgers in the same sync run,
    // mirroring the same-run handling for checked/canceled tasks.
    for terminal in ["done", "canceled"] {
        let reopened = parse_clean_project(
            "Child.md",
            &format!(
                "---\ntype: [[project]]\nstatus: {terminal}\n---\n- [ ] #task Ship child #hide ^prj\n"
            ),
        );
        assert_eq!(
            SubprojectState::from_project(&reopened),
            Some(SubprojectState::Open),
            "terminal status {terminal} with an open ^prj should be open"
        );
    }

    // A terminal child whose ^prj task is missing stays terminal.
    let closed = parse_clean_project(
        "Closed.md",
        "---\ntype: [[project]]\nstatus: done\n---\n- [ ] #task Needs completion task\n",
    );
    assert_eq!(
        SubprojectState::from_project(&closed),
        Some(SubprojectState::Done)
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
    assert!(plan_project_sync(&already_on_dash, &[]).changes.is_empty());

    issues.clear();
    let already_hidden = parse_project(
        Path::new("AlreadyHidden.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n- [ ] #task Needs surfacing\n",
        &mut issues,
    )
    .expect("project");
    assert!(plan_project_sync(&already_hidden, &[]).changes.is_empty());
    assert!(issues.is_empty());
}

#[test]
fn subproject_aggregation_marks_only_schedules_after_shared_today() {
    let project = |name: &str, scheduled: Option<&str>, task: &str| {
        let scheduled = scheduled
            .map(|date| format!("scheduled: {date}\n"))
            .unwrap_or_default();
        parse_clean_project(
            &format!("{name}.md"),
            &format!(
                "---\ntype: [[project]]\nparent: [[Parent]]\n{scheduled}---\n{task}\n"
            ),
        )
    };
    let tomorrow = project(
        "Tomorrow",
        Some("2026-07-11"),
        "- [ ] #task Ship tomorrow #hide ^prj",
    );
    let today =
        project("Today", Some("2026-07-10"), "- [ ] #task Ship today ^prj");
    let past =
        project("Past", Some("2026-07-09"), "- [ ] #task Ship past ^prj");
    let absent = project("Absent", None, "- [ ] #task Ship absent ^prj");
    let closed_future = project(
        "ClosedFuture",
        Some("2026-07-12"),
        "- [x] #task Ship closed #hide ^prj",
    );

    let children = subproject_children_by_parent_link_name(
        [&tomorrow, &today, &past, &absent, &closed_future],
        NaiveDate::from_ymd_opt(2026, 7, 10).unwrap(),
    );
    assert_eq!(
        children.get("parent"),
        Some(&vec![
            open_subproject("Absent"),
            future_subproject("ClosedFuture", SubprojectState::Done),
            open_subproject("Past"),
            open_subproject("Today"),
            future_subproject("Tomorrow", SubprojectState::Open),
        ])
    );
}

#[test]
fn project_sync_plan_removes_stale_scheduled_field() {
    let mut issues = Vec::new();
    let project = parse_project(
        Path::new("Scheduled.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide [scheduled::2026-06-01] ^prj\n",
        &mut issues,
    )
    .expect("project");

    assert!(issues.is_empty());
    assert_eq!(
        plan_project_sync(&project, &[]).changes,
        vec![
            ProjectChange::RemoveHideTag,
            ProjectChange::RemoveScheduled {
                scheduled: "2026-06-01".to_string(),
            },
        ]
    );
}

#[test]
fn project_sync_plan_reopens_terminal_project_from_open_prj() {
    for terminal in ["done", "canceled"] {
        let contents = format!(
            "---\ntype: [[project]]\nstatus: {terminal}\n---\n- [ ] #task Ship #hide ^prj\n"
        );
        let project = parse_clean_project("Reopened.md", &contents);
        let plan = plan_project_sync(&project, &[]);

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
        let plan = plan_project_sync(&project, &[]);

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
    let plan = plan_project_sync(&project, &[]);

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

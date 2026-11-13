//! Sync planning, subproject aggregation, and sync walk.
use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SyncEvent {
    Status {
        project_name: String,
        from: String,
        to: String,
        reason: String,
    },
    PrjEdit {
        project_name: String,
        action: PrjEditAction,
        field: String,
        reason: String,
    },
    TaskSchedules {
        project_name: String,
        scheduled: String,
        future: bool,
        scheduled_task_count: usize,
        removed_hide_count: usize,
        prj_hide_changed: bool,
    },
    HoistedSchedule {
        project_name: String,
        scheduled: String,
    },
    Warning {
        project_name: String,
        message: String,
        detail: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrjEditAction {
    Add,
    Remove,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct ProjectPlan {
    pub(super) changes: Vec<ProjectChange>,
    pub(super) warnings: Vec<SyncEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ProjectChange {
    Status {
        from: String,
        to: TargetProjectStatus,
    },
    RemoveHideTag,
    AddHideTag {
        reason: AddHideReason,
    },
    ReconcileTaskSchedules {
        scheduled: String,
        policy: TaskSchedulePolicy,
        scheduled_task_count: usize,
        removed_hide_count: usize,
        prj_hide_changed: bool,
    },
    HoistScheduled {
        scheduled: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AddHideReason {
    NonHiddenOpenTasks,
    OpenSubprojects,
}

impl AddHideReason {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::NonHiddenOpenTasks => "non-hidden open tasks exist",
            Self::OpenSubprojects => "project has open sub-projects",
        }
    }
}

impl ProjectChange {
    pub(super) fn event(&self, project_name: &str) -> SyncEvent {
        match self {
            Self::Status { from, to } => SyncEvent::Status {
                project_name: project_name.to_string(),
                from: from.clone(),
                to: to.label().to_string(),
                reason: to.reason().to_string(),
            },
            Self::RemoveHideTag => SyncEvent::PrjEdit {
                project_name: project_name.to_string(),
                action: PrjEditAction::Remove,
                field: HIDE_TAG.to_string(),
                reason: "no non-hidden open tasks or open sub-projects"
                    .to_string(),
            },
            Self::AddHideTag { reason } => SyncEvent::PrjEdit {
                project_name: project_name.to_string(),
                action: PrjEditAction::Add,
                field: HIDE_TAG.to_string(),
                reason: reason.label().to_string(),
            },
            Self::ReconcileTaskSchedules {
                scheduled,
                policy,
                scheduled_task_count,
                removed_hide_count,
                prj_hide_changed,
            } => SyncEvent::TaskSchedules {
                project_name: project_name.to_string(),
                scheduled: scheduled.clone(),
                future: policy.future,
                scheduled_task_count: *scheduled_task_count,
                removed_hide_count: *removed_hide_count,
                prj_hide_changed: *prj_hide_changed,
            },
            Self::HoistScheduled { scheduled } => SyncEvent::HoistedSchedule {
                project_name: project_name.to_string(),
                scheduled: scheduled.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct InlineFieldSpan {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) value_start: usize,
    pub(super) value_end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TextEdit {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) replacement: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LineSpan {
    pub(super) line_number: usize,
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) next_start: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FrontmatterLayout {
    pub(super) body_start_line: usize,
    pub(super) type_line: Option<LineSpan>,
    pub(super) status_line: Option<LineSpan>,
}

/// Sync's effective-unhidden count for the surfacing rule, shared with
/// the project pull-in surface predicate
/// (`docs/task-dependencies.md` §12.10 REMAINING). Without a project
/// schedule this is the scan's open-unhidden count; with one, a hidden
/// ordinary open task with fewer than two scheduled fields counts too,
/// because sync strips its `#hide` on this run.
pub(super) fn effective_unhidden_count(
    project: &Project,
    sync_schedule: Option<&ProjectSchedule>,
    today: NaiveDate,
) -> usize {
    let Some(scheduled) = sync_schedule else {
        return project.open_unhidden_count;
    };
    let policy = TaskSchedulePolicy::for_schedule(scheduled.date, today);
    project
        .task_lines
        .iter()
        .filter(|task| {
            task.is_open_task
                && !task.is_prj
                && (task.hide_tag_count == 0
                    || policy.ordinary_hide_needs_removal(**task))
        })
        .count()
}

/// Sync's surfacing rule, shared with the pull-in EMPTY predicate: no
/// remaining work and no open sub-project. Sync passes child-note states;
/// the pull-in passes the marker-line states.
pub(super) fn should_surface_project(
    effective_unhidden: usize,
    has_open_subprojects: bool,
) -> bool {
    effective_unhidden == 0 && !has_open_subprojects
}

pub(super) fn plan_project_sync_at(
    project: &Project,
    has_open_subprojects: bool,
    today: NaiveDate,
) -> ProjectPlan {
    let mut plan = ProjectPlan::default();

    if project.prj_task.state == PrjTaskState::Missing
        && !project.status.is_terminal()
    {
        plan.warnings.push(SyncEvent::Warning {
            project_name: project.name.clone(),
            message: "active project has no ^prj task".to_string(),
            detail: format!("add `{PROJECT_TASK_SHAPE}`"),
        });
    }

    if project.prj_task.placeholder {
        plan.warnings.push(SyncEvent::Warning {
            project_name: project.name.clone(),
            message: "^prj task still uses the template placeholder"
                .to_string(),
            detail: "replace it with concrete completion criteria".to_string(),
        });
    }

    let mut effective_status = project.status.clone();
    if let Some(target) = project.prj_task.target_status(&project.status)
        && !target.matches(&project.status)
    {
        plan.changes.push(ProjectChange::Status {
            from: project.status.label().to_string(),
            to: target,
        });
        effective_status = target.as_project_status();
    }

    // Effective post-reconcile unhidden count for the surfacing rule
    // below. When a schedule policy applies, an open ordinary task that
    // is hidden now but will be unhidden this run counts as unhidden.
    if !effective_status.is_terminal()
        && let Some(scheduled) = &project.scheduled
    {
        if project.prj_task.state == PrjTaskState::Open {
            plan.changes.push(ProjectChange::HoistScheduled {
                scheduled: scheduled.raw.clone(),
            });
        } else {
            plan.warnings.push(SyncEvent::Warning {
                project_name: project.name.clone(),
                message: "cannot hoist leftover scheduled onto ^prj"
                    .to_string(),
                detail: "frontmatter scheduled left in place; hoist requires an open ^prj task"
                    .to_string(),
            });
        }
    }

    let sync_schedule = project_schedule_for_sync(project, &effective_status);
    let mut effective_unhidden = project.open_unhidden_count;
    if let Some(scheduled) = &sync_schedule {
        let policy = TaskSchedulePolicy::for_schedule(scheduled.date, today);
        effective_unhidden =
            effective_unhidden_count(project, Some(scheduled), today);
        let scheduled_task_count = project
            .task_lines
            .iter()
            .filter(|task| policy.ordinary_schedule_needs_change(**task))
            .count();
        let removed_hide_count = project
            .task_lines
            .iter()
            .filter(|task| policy.ordinary_hide_needs_removal(**task))
            .count();
        let prj_hide_changed = project
            .task_lines
            .iter()
            .copied()
            .any(|task| policy.prj_hide_needs_change(task));
        for task in project
            .task_lines
            .iter()
            .filter(|task| !task.is_prj && task.scheduled_field_count > 1)
        {
            plan.warnings.push(SyncEvent::Warning {
                project_name: project.name.clone(),
                message: "task has multiple scheduled fields".to_string(),
                detail: format!(
                    "line {} left unchanged; keep exactly one",
                    task.line_number
                ),
            });
        }
        if scheduled_task_count > 0
            || removed_hide_count > 0
            || prj_hide_changed
        {
            plan.changes.push(ProjectChange::ReconcileTaskSchedules {
                scheduled: scheduled.raw.clone(),
                policy,
                scheduled_task_count,
                removed_hide_count,
                prj_hide_changed,
            });
        }
    }

    if !effective_status.is_terminal()
        && project.prj_task.state == PrjTaskState::Open
    {
        let should_surface =
            should_surface_project(effective_unhidden, has_open_subprojects);
        if sync_schedule.as_ref().is_none_or(|s| s.date <= today) {
            if should_surface {
                if project.prj_task.hidden {
                    plan.changes.push(ProjectChange::RemoveHideTag);
                }
            } else if !project.prj_task.hidden {
                let reason = if effective_unhidden > 0 {
                    AddHideReason::NonHiddenOpenTasks
                } else {
                    AddHideReason::OpenSubprojects
                };
                plan.changes.push(ProjectChange::AddHideTag { reason });
            }
        }
    }

    plan
}

pub(super) fn prj_inline_schedule_date(project: &Project) -> Option<NaiveDate> {
    project
        .task_lines
        .iter()
        .find(|task| task.is_prj)
        .and_then(|task| task.scheduled_date)
}

pub(super) fn project_schedule_for_sync(
    project: &Project,
    effective_status: &ProjectStatus,
) -> Option<ProjectSchedule> {
    if effective_status.is_terminal() {
        return None;
    }
    // The frontmatter value is only a schedule source when this pass
    // hoists it onto an open ^prj. Without that write, only an inline
    // ^prj date may be propagated to ordinary tasks.
    if project.prj_task.state == PrjTaskState::Open
        && let Some(scheduled) = &project.scheduled
    {
        return Some(scheduled.clone());
    }
    let date = prj_inline_schedule_date(project)?;
    Some(ProjectSchedule {
        raw: date.format("%Y-%m-%d").to_string(),
        date,
    })
}

pub(crate) fn subproject_displays_in_marker_line(
    line: &str,
) -> HashMap<String, SubprojectDisplay> {
    let mut displays = HashMap::new();
    for span in wikilink_spans_in_line(line) {
        displays.entry(span.link.link_name).or_insert_with(|| {
            display_for_wikilink(line, span.start, span.end)
        });
    }
    displays
}

pub(crate) fn display_for_wikilink(
    line: &str,
    link_start: usize,
    link_end: usize,
) -> SubprojectDisplay {
    let before_link = line[..link_start].trim_end();
    let after_link = line[link_end..].trim_start();
    let (before_entry, state) = if let Some(before_strike) =
        before_link.strip_suffix("~~")
        && let Some(after_strike) = after_link.strip_prefix("~~")
    {
        let marker = after_strike.trim_start();
        let state = if marker.starts_with(SUBPROJECT_DONE_MARKER) {
            SubprojectState::Done
        } else if marker.starts_with(SUBPROJECT_CANCELED_MARKER) {
            SubprojectState::Canceled
        } else {
            SubprojectState::Open
        };
        (before_strike.trim_end(), state)
    } else {
        (before_link, SubprojectState::Open)
    };

    SubprojectDisplay {
        state,
        future_scheduled: before_entry
            .ends_with(SUBPROJECT_FUTURE_SCHEDULE_MARKER),
    }
}

pub(super) fn is_direct_prj_child(
    sub_block: &PrjSubBlock,
    line: &PrjSubBlockLine,
) -> bool {
    if line.indentation.len() <= sub_block.prj_indent.len()
        || !line.indentation.starts_with(&sub_block.prj_indent)
    {
        return false;
    }
    !sub_block.lines.iter().any(|previous| {
        previous.line_number < line.line_number
            && line.indentation.starts_with(&previous.indentation)
            && line.indentation.len() > previous.indentation.len()
    })
}

pub(super) fn subprojects_subtree_end<'a>(
    sub_block: &'a PrjSubBlock,
    anchor: &'a PrjSubBlockLine,
) -> &'a PrjSubBlockLine {
    let mut last = anchor;
    for line in &sub_block.lines {
        if line.line_number <= anchor.line_number {
            continue;
        }
        if line.indentation.len() <= anchor.indentation.len()
            || !line.indentation.starts_with(&anchor.indentation)
        {
            break;
        }
        last = line;
    }
    last
}

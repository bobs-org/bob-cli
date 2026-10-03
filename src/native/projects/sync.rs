//! Sync planning, subproject aggregation, and sync walk.
use super::*;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct SyncReport {
    pub(super) project_count: usize,
    pub(super) events: Vec<SyncEvent>,
    pub(super) issues: Vec<ScanIssue>,
}

impl SyncReport {
    pub(super) fn status_update_count(&self) -> usize {
        self.events
            .iter()
            .filter(|event| matches!(event, SyncEvent::Status { .. }))
            .count()
    }

    pub(super) fn prj_edit_count(&self) -> usize {
        self.events
            .iter()
            .filter(|event| matches!(event, SyncEvent::PrjEdit { .. }))
            .count()
    }

    pub(super) fn task_schedule_count(&self) -> usize {
        self.events
            .iter()
            .filter_map(|event| match event {
                SyncEvent::TaskSchedules {
                    scheduled_task_count,
                    ..
                } => Some(*scheduled_task_count),
                _ => None,
            })
            .sum()
    }

    pub(super) fn warning_count(&self) -> usize {
        self.events
            .iter()
            .filter(|event| matches!(event, SyncEvent::Warning { .. }))
            .count()
    }
}

#[derive(Debug, Clone)]
pub(super) struct SyncFile {
    pub(super) path: PathBuf,
    pub(super) contents: String,
    pub(super) project: Project,
    pub(super) can_plan: bool,
    pub(super) can_be_subproject: bool,
}

impl SyncFile {
    pub(super) fn clean_project(&self) -> Option<&Project> {
        self.can_be_subproject.then_some(&self.project)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SyncEvent {
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
    Warning {
        project_name: String,
        message: String,
        detail: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PrjEditAction {
    Add,
    Remove,
    Update,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct ProjectPlan {
    pub(super) changes: Vec<ProjectChange>,
    pub(super) warnings: Vec<SyncEvent>,
    pub(super) desired_subprojects: Option<Vec<SubprojectEntry>>,
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
    RemoveScheduled {
        scheduled: String,
    },
    AddSubprojectLink {
        stem: String,
        state: SubprojectState,
    },
    RemoveSubprojectLink {
        stem: String,
    },
    MarkSubproject {
        stem: String,
        state: SubprojectState,
    },
    AddSubprojectScheduleMarker {
        stem: String,
    },
    RemoveSubprojectScheduleMarker {
        stem: String,
    },
    NormalizeSubprojects,
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
            Self::RemoveScheduled { scheduled } => SyncEvent::PrjEdit {
                project_name: project_name.to_string(),
                action: PrjEditAction::Remove,
                field: format!("[scheduled::{scheduled}]"),
                reason: "scheduled is no longer used".to_string(),
            },
            Self::AddSubprojectLink { stem, state } => SyncEvent::PrjEdit {
                project_name: project_name.to_string(),
                action: PrjEditAction::Add,
                field: format!("[[{stem}]]"),
                reason: state.reason().to_string(),
            },
            Self::RemoveSubprojectLink { stem, .. } => SyncEvent::PrjEdit {
                project_name: project_name.to_string(),
                action: PrjEditAction::Remove,
                field: format!("[[{stem}]]"),
                reason: "no longer a sub-project".to_string(),
            },
            Self::MarkSubproject { stem, state } => SyncEvent::PrjEdit {
                project_name: project_name.to_string(),
                action: PrjEditAction::Update,
                field: format!("[[{stem}]]"),
                reason: state.reason().to_string(),
            },
            Self::AddSubprojectScheduleMarker { stem } => SyncEvent::PrjEdit {
                project_name: project_name.to_string(),
                action: PrjEditAction::Add,
                field: format!(
                    "{SUBPROJECT_FUTURE_SCHEDULE_MARKER} [[{stem}]]"
                ),
                reason: "sub-project scheduled in future".to_string(),
            },
            Self::RemoveSubprojectScheduleMarker { stem } => {
                SyncEvent::PrjEdit {
                    project_name: project_name.to_string(),
                    action: PrjEditAction::Remove,
                    field: format!(
                        "{SUBPROJECT_FUTURE_SCHEDULE_MARKER} [[{stem}]]"
                    ),
                    reason: "sub-project no longer scheduled in future"
                        .to_string(),
                }
            }
            Self::NormalizeSubprojects => SyncEvent::PrjEdit {
                project_name: project_name.to_string(),
                action: PrjEditAction::Update,
                field: "sub-projects".to_string(),
                reason: "canonical format".to_string(),
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

pub(super) fn sync_projects(bob_dir: &Path, dry_run: bool) -> SyncReport {
    let mut report = SyncReport::default();
    let mut files = Vec::new();
    let today = bob_env::current_datetime().date();
    collect_sync_directory(bob_dir, bob_dir, &mut report, &mut files);
    let subproject_children = subproject_children_by_parent_link_name(
        files.iter().filter_map(SyncFile::clean_project),
        today,
    );
    apply_sync_plans(&files, &subproject_children, today, dry_run, &mut report);
    report
}

pub(super) fn collect_sync_directory(
    root: &Path,
    directory: &Path,
    report: &mut SyncReport,
    files: &mut Vec<SyncFile>,
) {
    let entries = match read_sorted_directory(directory) {
        Ok(entries) => entries,
        Err(error) => {
            report.issues.push(ScanIssue::path(
                relative_or_original(root, directory),
                format!("failed to read directory: {error}"),
            ));
            return;
        }
    };

    for entry in entries {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                report.issues.push(ScanIssue::path(
                    relative_or_original(root, &path),
                    format!("failed to inspect path: {error}"),
                ));
                continue;
            }
        };

        if file_type.is_dir() {
            if is_excluded_directory(&path) {
                continue;
            }
            collect_sync_directory(root, &path, report, files);
            continue;
        }

        if file_type.is_file() && is_markdown_file(&path) {
            collect_sync_markdown_file(root, &path, report, files);
        }
    }
}

pub(super) fn collect_sync_markdown_file(
    root: &Path,
    path: &Path,
    report: &mut SyncReport,
    files: &mut Vec<SyncFile>,
) {
    let relative_path = relative_or_original(root, path);
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) => {
            report.issues.push(ScanIssue::path(
                relative_path,
                format!("failed to read file: {error}"),
            ));
            return;
        }
    };

    let issue_count = report.issues.len();
    let Some(project) =
        parse_project(&relative_path, &contents, &mut report.issues)
    else {
        return;
    };
    report.project_count += 1;
    let can_plan = report.issues.len() == issue_count;
    let can_be_subproject =
        can_plan || issues_allow_subproject(&report.issues[issue_count..]);
    files.push(SyncFile {
        path: path.to_path_buf(),
        contents,
        project,
        can_plan,
        can_be_subproject,
    });
}

pub(super) fn issues_allow_subproject(issues: &[ScanIssue]) -> bool {
    !issues.is_empty()
        && issues.iter().all(|issue| {
            issue.message.starts_with("scheduled ")
                || issue.message.starts_with("multiple scheduled ")
        })
}

pub(super) fn apply_sync_plans(
    files: &[SyncFile],
    subproject_children: &HashMap<String, Vec<SubprojectEntry>>,
    today: NaiveDate,
    dry_run: bool,
    report: &mut SyncReport,
) {
    for file in files {
        if !file.can_plan {
            continue;
        }

        let children = subproject_children
            .get(&file.project.link_name)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let plan = plan_project_sync_at(&file.project, children, today);
        report.events.extend(plan.warnings);

        if plan.changes.is_empty() {
            continue;
        }

        if !dry_run {
            let new_contents = match apply_project_changes(
                &file.contents,
                &plan.changes,
                plan.desired_subprojects.as_deref(),
            ) {
                Ok(new_contents) => new_contents,
                Err(message) => {
                    report.issues.push(ScanIssue::path(
                        file.project.relative_path.clone(),
                        message,
                    ));
                    continue;
                }
            };

            if let Err(error) = fs::write(&file.path, new_contents) {
                report.issues.push(ScanIssue::path(
                    file.project.relative_path.clone(),
                    format!("failed to write file: {error}"),
                ));
                continue;
            }
        }

        for change in &plan.changes {
            report.events.push(change.event(&file.project.name));
        }
    }
}

pub(super) fn subproject_children_by_parent_link_name<'a>(
    projects: impl IntoIterator<Item = &'a Project>,
    today: NaiveDate,
) -> HashMap<String, Vec<SubprojectEntry>> {
    let mut children_by_parent: HashMap<
        String,
        BTreeMap<String, SubprojectEntry>,
    > = HashMap::new();

    for project in projects {
        let Some(state) = SubprojectState::from_project(project) else {
            continue;
        };
        let Some(parent) = &project.parent_target else {
            continue;
        };
        if parent == &project.link_name {
            continue;
        }
        children_by_parent
            .entry(parent.clone())
            .or_default()
            .entry(project.link_name.clone())
            .or_insert_with(|| SubprojectEntry {
                link_name: project.link_name.clone(),
                stem: project.link_stem.clone(),
                state,
                future_scheduled: project
                    .scheduled
                    .as_ref()
                    .is_some_and(|scheduled| scheduled.date > today),
            });
    }

    children_by_parent
        .into_iter()
        .map(|(parent, children)| {
            (parent, children.into_values().collect::<Vec<_>>())
        })
        .collect()
}

pub(super) fn plan_project_sync_at(
    project: &Project,
    subproject_children: &[SubprojectEntry],
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
    let mut effective_unhidden = project.open_unhidden_count;
    if !effective_status.is_terminal()
        && let Some(scheduled) = &project.scheduled
    {
        let policy = TaskSchedulePolicy::for_schedule(scheduled.date, today);
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
        effective_unhidden = project
            .task_lines
            .iter()
            .filter(|task| {
                task.is_open_task
                    && !task.is_prj
                    && (task.hide_tag_count == 0
                        || policy.ordinary_hide_needs_removal(**task))
            })
            .count();
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
        let has_open_subprojects = subproject_children
            .iter()
            .any(|child| child.state.is_open());
        let should_surface = effective_unhidden == 0 && !has_open_subprojects;
        if project.scheduled.as_ref().is_none_or(|s| s.date <= today) {
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

        let marker_line = project.prj_task.sub_block.first_marker_line();
        let mut marker_targets = HashSet::new();
        let marker_links = marker_line
            .map(|line| {
                line.links
                    .iter()
                    .filter(|link| {
                        marker_targets.insert(link.link_name.clone())
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let marker_displays = marker_line
            .map(|line| subproject_displays_in_marker_line(&line.trimmed_text))
            .unwrap_or_default();
        let desired_subprojects =
            desired_subproject_entries(subproject_children, &marker_targets);
        let desired_targets = desired_subprojects
            .iter()
            .map(|entry| entry.link_name.clone())
            .collect::<HashSet<_>>();
        let subproject_change_start = plan.changes.len();

        for entry in &desired_subprojects {
            if !marker_targets.contains(&entry.link_name) {
                plan.changes.push(ProjectChange::AddSubprojectLink {
                    stem: entry.stem.clone(),
                    state: entry.state,
                });
            }
        }

        for link in &marker_links {
            if !desired_targets.contains(&link.link_name) {
                plan.changes.push(ProjectChange::RemoveSubprojectLink {
                    stem: link.stem.clone(),
                });
            }
        }

        for entry in &desired_subprojects {
            if !marker_targets.contains(&entry.link_name) {
                continue;
            }
            let display = marker_displays
                .get(&entry.link_name)
                .copied()
                .unwrap_or(SubprojectDisplay {
                    state: SubprojectState::Open,
                    future_scheduled: false,
                });
            if display.state != entry.state {
                plan.changes.push(ProjectChange::MarkSubproject {
                    stem: entry.stem.clone(),
                    state: entry.state,
                });
            }
            if display.future_scheduled != entry.future_scheduled {
                let change = if entry.future_scheduled {
                    ProjectChange::AddSubprojectScheduleMarker {
                        stem: entry.stem.clone(),
                    }
                } else {
                    ProjectChange::RemoveSubprojectScheduleMarker {
                        stem: entry.stem.clone(),
                    }
                };
                plan.changes.push(change);
            }
        }

        if plan.changes.len() == subproject_change_start
            && subprojects_need_normalization(
                &project.prj_task.sub_block,
                &desired_subprojects,
            )
        {
            plan.changes.push(ProjectChange::NormalizeSubprojects);
        }
        if plan.changes.len() > subproject_change_start {
            plan.desired_subprojects = Some(desired_subprojects);
        }

        if let Some(scheduled) = &project.prj_task.scheduled {
            plan.changes.push(ProjectChange::RemoveScheduled {
                scheduled: scheduled.clone(),
            });
        }
    }

    plan
}

pub(super) fn desired_subproject_entries(
    children: &[SubprojectEntry],
    marker_targets: &HashSet<String>,
) -> Vec<SubprojectEntry> {
    let mut open_entries = children
        .iter()
        .filter(|child| child.state.is_open())
        .cloned()
        .collect::<Vec<_>>();
    let mut closed_entries = children
        .iter()
        .filter(|child| {
            child.state.is_terminal()
                && marker_targets.contains(&child.link_name)
        })
        .cloned()
        .collect::<Vec<_>>();

    open_entries.sort_by(|left, right| left.link_name.cmp(&right.link_name));
    closed_entries.sort_by(|left, right| left.link_name.cmp(&right.link_name));
    open_entries.extend(closed_entries);
    open_entries
}

pub(super) fn subproject_displays_in_marker_line(
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

pub(super) fn display_for_wikilink(
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

pub(super) fn subprojects_need_normalization(
    sub_block: &PrjSubBlock,
    desired_entries: &[SubprojectEntry],
) -> bool {
    let marker_count = sub_block.marker_line_count();
    if desired_entries.is_empty() {
        return marker_count > 0;
    }

    let Some(marker_line) = sub_block.first_marker_line() else {
        return false;
    };
    let expected_indent = format!("{}\t", sub_block.prj_indent);
    let expected_text = render_subprojects_line_text(desired_entries);
    marker_count > 1
        || marker_line.indentation != expected_indent
        || marker_line.trimmed_text != expected_text
}

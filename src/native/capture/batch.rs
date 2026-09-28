//! Staged batch planning and shared plan/detail types.
use super::*;

#[derive(Default)]
pub(super) struct CaptureBatchPlanner {
    pub(super) files: Vec<BatchTextFile>,
    pub(super) by_path: HashMap<PathBuf, usize>,
}

pub(super) struct BatchTextFile {
    pub(super) path: PathBuf,
    pub(super) existed: bool,
    pub(super) present: bool,
    pub(super) original: String,
    pub(super) current: String,
}

pub(super) struct StagedTextFile {
    pub(super) target: PathBuf,
    pub(super) target_existed: bool,
    pub(super) original_target: String,
    pub(super) updated_target: String,
}

impl CaptureBatchPlanner {
    pub(super) fn currently_exists(
        &mut self,
        path: &Path,
    ) -> Result<bool, CaptureError> {
        let index = self.ensure_loaded(path)?;
        Ok(self.files[index].present)
    }

    pub(super) fn read_existing(
        &mut self,
        path: &Path,
    ) -> Result<String, CaptureError> {
        let index = self.ensure_loaded(path)?;
        if !self.files[index].present {
            return Err(CaptureError::io(format!(
                "note does not exist: {}",
                path.display()
            )));
        }
        Ok(self.files[index].current.clone())
    }

    pub(super) fn current_contents(
        &mut self,
        path: &Path,
    ) -> Result<Option<String>, CaptureError> {
        let index = self.ensure_loaded(path)?;
        Ok(self.files[index]
            .present
            .then(|| self.files[index].current.clone()))
    }

    pub(super) fn staged_snapshot(&self) -> BTreeMap<PathBuf, Option<String>> {
        self.files
            .iter()
            .map(|file| {
                (
                    file.path.clone(),
                    file.present.then(|| file.current.clone()),
                )
            })
            .collect()
    }

    pub(super) fn stage(
        &mut self,
        path: &Path,
        updated: String,
    ) -> Result<(), CaptureError> {
        let index = self.ensure_loaded(path)?;
        self.files[index].present = true;
        self.files[index].current = updated;
        Ok(())
    }

    pub(super) fn ensure_loaded(
        &mut self,
        path: &Path,
    ) -> Result<usize, CaptureError> {
        if let Some(index) = self.by_path.get(path) {
            return Ok(*index);
        }

        let present = path.exists();
        let original = if present {
            read_target(path)?
        } else {
            String::new()
        };
        let index = self.files.len();
        self.by_path.insert(path.to_path_buf(), index);
        self.files.push(BatchTextFile {
            path: path.to_path_buf(),
            existed: present,
            present,
            current: original.clone(),
            original,
        });
        Ok(index)
    }

    pub(super) fn into_staged_files(self) -> Vec<StagedTextFile> {
        self.files
            .into_iter()
            .filter(|file| {
                file.present && (!file.existed || file.current != file.original)
            })
            .map(|file| StagedTextFile {
                target: file.path,
                target_existed: file.existed,
                original_target: file.original,
                updated_target: file.current,
            })
            .collect()
    }
}

pub(super) fn plan_capture_to_target(
    planner: &mut CaptureBatchPlanner,
    target: &Path,
    capture_block: &str,
    kind: &CaptureKind,
) -> Result<CaptureWritePlan, CaptureError> {
    if !planner.currently_exists(target)? {
        validate_target_parent(target)?;
        let updated_target = format!("{capture_block}\n");
        planner.stage(target, updated_target)?;
        return Ok(CaptureWritePlan {
            placement: Placement::Created,
            pomodoro: None,
            sub_bullet: None,
            pomodoro_note: None,
            toggle: None,
            pomodoro_link: None,
        });
    }

    let contents = planner.read_existing(target)?;
    if let CaptureKind::TaskWithBlockId { block_id } = kind {
        reject_duplicate_block_id(&contents, block_id, target)?;
    }
    let (updated, placement) = match kind {
        CaptureKind::Task
        | CaptureKind::TaskWithBlockId { .. }
        | CaptureKind::Pomodoro { .. } => {
            insert_task_line(&contents, capture_block)
        }
        CaptureKind::Bullet {
            section_prefix,
            exact,
        } => insert_bullet_line(
            &contents,
            capture_block,
            section_prefix.as_deref(),
            *exact,
        ),
        CaptureKind::ProjectNote { .. } => {
            return Err(CaptureError::io(
                "project-note capture invariant failed: wrong write planner",
            ));
        }
        CaptureKind::SubBullet { .. } => {
            return Err(CaptureError::io(
                "sub-bullet capture invariant failed: wrong write planner",
            ));
        }
        CaptureKind::PomodoroNote => {
            return Err(CaptureError::io(
                "pomodoro-note capture invariant failed: wrong write planner",
            ));
        }
        CaptureKind::TaskToggle { .. } => {
            return Err(CaptureError::io(
                "task toggle capture invariant failed: wrong write planner",
            ));
        }
        CaptureKind::PomodoroAdjust { .. } => {
            return Err(CaptureError::io(
                "pomodoro adjustment capture invariant failed: wrong write planner",
            ));
        }
        CaptureKind::PomodoroShift { .. } => {
            return Err(CaptureError::io(
                "pomodoro shift capture invariant failed: wrong write planner",
            ));
        }
        CaptureKind::PomodoroLink { .. } => {
            return Err(CaptureError::io(
                "pomodoro link capture invariant failed: wrong write planner",
            ));
        }
        CaptureKind::PomodoroClose { .. } => {
            return Err(CaptureError::io(
                "pomodoro close capture invariant failed: wrong write planner",
            ));
        }
        CaptureKind::PomodoroStart { .. } => {
            return Err(CaptureError::io(
                "pomodoro start capture invariant failed: wrong write planner",
            ));
        }
    };
    planner.stage(target, updated)?;
    Ok(CaptureWritePlan {
        placement,
        pomodoro: None,
        sub_bullet: None,
        pomodoro_note: None,
        toggle: None,
        pomodoro_link: None,
    })
}

#[derive(Debug)]
pub(super) struct CaptureWritePlan {
    pub(super) placement: Placement,
    pub(super) pomodoro: Option<PlannedPomodoroEdit>,
    pub(super) sub_bullet: Option<SubBulletCaptureDetails>,
    pub(super) pomodoro_note: Option<PomodoroNoteDetails>,
    pub(super) toggle: Option<TaskToggleCaptureDetails>,
    pub(super) pomodoro_link: Option<PomodoroLinkCaptureDetails>,
}

#[derive(Debug)]
pub(super) struct PomodoroLinkCaptureDetails {
    pub(super) previous_task_line: String,
    pub(super) task_line: String,
    pub(super) task_description: String,
    pub(super) previous_status_symbol: char,
    pub(super) previous_status_name: String,
    pub(super) status_symbol: char,
    pub(super) status_name: String,
    pub(super) block_id: String,
    pub(super) day_file: String,
    pub(super) block_link: String,
    pub(super) pomodoro_link_placement: Option<Placement>,
    pub(super) pomodoro_name: Option<String>,
    pub(super) creates_pomodoro: bool,
    pub(super) pomodoro_link_action: &'static str,
    pub(super) pomodoro_link_source: Option<PomodoroLinkEndpoint>,
    pub(super) pomodoro_link_destination: Option<PomodoroLinkEndpoint>,
    pub(super) removed_scheduled: Option<String>,
    pub(super) schedule_log: Option<capture_schedule_log::ScheduleLog>,
    pub(super) status_changed: bool,
    pub(super) pomodoro_start: Option<PomodoroStartSummary>,
}

#[derive(Debug)]
pub(super) struct SubBulletCaptureDetails {
    pub(super) block_id: Option<String>,
    pub(super) parent_line: usize,
    pub(super) parent_text: String,
    pub(super) parent_section: Option<String>,
    pub(super) parent_status_symbol: char,
    pub(super) parent_status_name: String,
}

#[derive(Debug)]
pub(super) struct PlannedPomodoroEdit {
    pub(super) details: PomodoroCaptureDetails,
    pub(super) start: Option<PomodoroStartSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroStartSummary {
    pub(super) start: String,
    pub(super) end: String,
    pub(super) duration_minutes: u64,
    pub(super) offset_units: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pomodoro_name: Option<String>,
    pub(super) pomodoro_line: usize,
    pub(super) created_pomodoro: bool,
    pub(super) time_range: String,
    /// Queued Task Link rows for a whole-item start only. `None` (omitted)
    /// for link and task starts, which stay byte-stable; whole-item starts
    /// always report it, possibly empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tasks: Option<Vec<PomodoroStartTaskJson>>,
}

/// One queued Task Link row on a started Pomodoro, following the close
/// row's explicit-null convention: unresolved rows carry `None` fields and
/// a `warning` instead of failing the start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroStartTaskJson {
    pub(super) block_link: String,
    pub(super) embedded: bool,
    pub(super) ledger_line: usize,
    pub(super) resolved: bool,
    pub(super) relative_target: Option<String>,
    pub(super) block_id: String,
    pub(super) text: Option<String>,
    pub(super) status_symbol: Option<char>,
    pub(super) status_name: Option<String>,
    pub(super) warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroAdjustSummary {
    pub(super) direction: &'static str,
    pub(super) requested_units: u64,
    pub(super) requested_minutes: i64,
    pub(super) delta_minutes: i64,
    pub(super) before_start: String,
    pub(super) before_end: String,
    pub(super) before_duration_minutes: u64,
    pub(super) after_start: String,
    pub(super) after_end: String,
    pub(super) after_duration_minutes: u64,
    pub(super) pomodoro_line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pomodoro_name: Option<String>,
    pub(super) time_range: String,
    pub(super) clamped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroShiftSummary {
    pub(super) direction: &'static str,
    pub(super) requested_units: u64,
    pub(super) delta_minutes: i64,
    pub(super) before_start: String,
    pub(super) before_end: String,
    pub(super) after_start: String,
    pub(super) after_end: String,
    pub(super) duration_minutes: u64,
    pub(super) pomodoro_line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pomodoro_name: Option<String>,
    pub(super) time_range: String,
}

#[derive(Debug)]
pub(super) struct PomodoroCaptureDetails {
    pub(super) block_id: String,
    pub(super) day_file: String,
    pub(super) block_link: String,
    pub(super) pomodoro_link_placement: Placement,
}

#[derive(Debug)]
pub(super) struct PomodoroNoteDetails {
    pub(super) day_file: String,
    pub(super) pomodoro_line: usize,
    pub(super) pomodoro_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TaskToggleDirection {
    Next,
    Open,
}

impl TaskToggleDirection {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Next => "next",
            Self::Open => "open",
        }
    }
}

#[derive(Debug)]
pub(super) struct TaskToggleCaptureDetails {
    pub(super) direction: TaskToggleDirection,
    pub(super) previous_task_line: String,
    pub(super) task_line: String,
    pub(super) task_description: String,
    pub(super) previous_status_symbol: char,
    pub(super) previous_status_name: String,
    pub(super) status_symbol: char,
    pub(super) status_name: String,
    pub(super) block_id: String,
    pub(super) day_file: String,
    pub(super) block_link: String,
    pub(super) pomodoro_link_placement: Option<Placement>,
    pub(super) pomodoro_name: Option<String>,
    pub(super) creates_pomodoro: bool,
    pub(super) pomodoro_already_linked: bool,
    pub(super) removed_pomodoro_links: usize,
    pub(super) removed_scheduled: Option<String>,
    pub(super) schedule_log: Option<capture_schedule_log::ScheduleLog>,
    pub(super) pomodoro_selector_unused: bool,
    pub(super) toggle_behavior: Option<&'static str>,
    pub(super) status_changed: Option<bool>,
    pub(super) pomodoro_link_action: Option<&'static str>,
    pub(super) pomodoro_link_source: Option<PomodoroLinkEndpoint>,
    pub(super) pomodoro_link_destination: Option<PomodoroLinkEndpoint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroLinkEndpoint {
    pub(super) line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) time_range: Option<String>,
}

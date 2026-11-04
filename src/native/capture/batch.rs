//! Staged batch planning and shared plan/detail types.
use super::*;

#[derive(Default)]
pub(crate) struct CaptureBatchPlanner {
    pub(super) files: Vec<BatchTextFile>,
    pub(super) by_path: HashMap<PathBuf, usize>,
    move_edges: Vec<(PathBuf, PathBuf)>,
    moved_origins: HashMap<(PathBuf, String), MovedOrigin>,
    item_pre_day: Option<Option<String>>,
    item_consumed_links: Vec<ConsumedLinkJson>,
    pub(super) read_guards: BatchReadGuards,
}

/// Inputs consulted by a merge even when they are not written by the batch.
#[derive(Default)]
pub(super) struct BatchReadGuards {
    files: BTreeMap<PathBuf, Option<String>>,
    scan_paths: Option<Vec<PathBuf>>,
}

impl BatchReadGuards {
    pub(super) fn validate(&self, bob_dir: &Path) -> Result<(), CaptureError> {
        if let Some(expected) = &self.scan_paths {
            let mut current = collect_done::link_repair_markdown_files(bob_dir)
                .map_err(|error| CaptureError::io(error.to_string()))?;
            current.sort();
            if &current != expected {
                return Err(CaptureError::io(
                    "vault Markdown membership changed after merge planning",
                )
                .with_typed(CaptureFailureKind::NoteChanged));
            }
        }
        for (path, expected) in &self.files {
            let current = match fs::read(path) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(CaptureError::io(error.to_string())),
            };
            if current.as_deref()
                != expected.as_ref().map(|text| text.as_bytes())
            {
                return Err(CaptureError::io(format!(
                    "merge input {} changed on disk after planning",
                    path.display()
                ))
                .with_typed(CaptureFailureKind::NoteChanged));
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
pub(super) struct MovedOrigin {
    pub(super) destination: PathBuf,
    pub(super) item_number: usize,
}

pub(super) struct BatchTextFile {
    pub(super) path: PathBuf,
    pub(super) existed: bool,
    pub(super) present: bool,
    pub(super) original: String,
    pub(super) current: String,
}

#[derive(Debug)]
pub(crate) struct StagedTextFile {
    pub(crate) target: PathBuf,
    pub(crate) target_existed: bool,
    pub(crate) original_target: String,
    pub(crate) updated_target: String,
}

impl CaptureBatchPlanner {
    pub(super) fn guard_merge_scan(
        &mut self,
        mut paths: Vec<PathBuf>,
    ) -> Result<(), CaptureError> {
        paths.sort();
        if self
            .read_guards
            .scan_paths
            .as_ref()
            .is_some_and(|previous| previous != &paths)
        {
            return Err(CaptureError::io(
                "vault Markdown membership changed during merge planning",
            )
            .with_typed(CaptureFailureKind::NoteChanged));
        }
        self.read_guards.scan_paths = Some(paths);
        Ok(())
    }

    pub(super) fn guard_merge_read(
        &mut self,
        path: &Path,
        snapshot: Option<String>,
    ) {
        let original = self
            .by_path
            .get(path)
            .map(|index| {
                let file = &self.files[*index];
                file.existed.then(|| file.original.clone())
            })
            .unwrap_or(snapshot);
        self.read_guards
            .files
            .entry(path.to_path_buf())
            .or_insert(original);
    }

    pub(super) fn begin_item(&mut self, pre_day: Option<String>) {
        self.item_pre_day = Some(pre_day);
        self.item_consumed_links.clear();
    }

    pub(super) fn item_pre_day(&self, path: &Path) -> Option<String> {
        match &self.item_pre_day {
            Some(Some(contents)) => Some(contents.clone()),
            Some(None) => self
                .by_path
                .get(path)
                .map(|index| self.files[*index].original.clone()),
            None => None,
        }
    }

    pub(super) fn record_consumed_links(
        &mut self,
        links: impl IntoIterator<Item = ConsumedLinkJson>,
    ) {
        self.item_consumed_links.extend(links);
    }

    pub(super) fn take_consumed_links(&mut self) -> Vec<ConsumedLinkJson> {
        std::mem::take(&mut self.item_consumed_links)
    }

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

    /// Read from the batch byte snapshot through the planner's overlay.
    /// Existing staged content wins; otherwise the captured disk text becomes
    /// the exact preimage for any later staged write, avoiding a second disk
    /// read while preserving the commit-time drift check.
    pub(super) fn current_contents_from_snapshot(
        &mut self,
        path: &Path,
        snapshot: Option<String>,
    ) -> Result<Option<String>, CaptureError> {
        if let Some(index) = self.by_path.get(path).copied() {
            let file = &self.files[index];
            return Ok(file.present.then(|| file.current.clone()));
        }
        let Some(contents) = snapshot else {
            return Ok(None);
        };
        let index = self.files.len();
        self.by_path.insert(path.to_path_buf(), index);
        self.files.push(BatchTextFile {
            path: path.to_path_buf(),
            existed: true,
            present: true,
            original: contents.clone(),
            current: contents.clone(),
        });
        Ok(Some(contents))
    }

    /// Non-loading peek at a file's staged text: `None` when the file
    /// was never touched (so batches that never touch the day file gain
    /// no new error path) or does not exist.
    pub(super) fn peek_text(&self, path: &Path) -> Option<String> {
        let index = *self.by_path.get(path)?;
        let file = &self.files[index];
        file.present.then(|| file.current.clone())
    }

    /// Original and current text for an already-loaded file that exists.
    pub(super) fn loaded_texts(&self, path: &Path) -> Option<(String, String)> {
        let index = *self.by_path.get(path)?;
        let file = &self.files[index];
        file.present
            .then(|| (file.original.clone(), file.current.clone()))
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

    /// Stage a move-engine postimage bound to the exact snapshot the
    /// planner consumed. Already-staged files keep their original disk
    /// preimage and current overlay; newly staged files adopt the
    /// snapshot's existence/content as their preimage. Any disk drift
    /// from the consumed snapshot is the existing typed `NoteChanged`
    /// refusal, so the commit's preimage guard can never be bypassed
    /// by a second disk read adopting an intervening edit.
    pub(super) fn stage_snapshot_postimage(
        &mut self,
        path: &Path,
        snapshot_contents: Option<String>,
        postimage: String,
    ) -> Result<(), CaptureError> {
        if self.by_path.contains_key(path) {
            let index = self.by_path[path];
            self.files[index].present = true;
            self.files[index].current = postimage;
            return Ok(());
        }
        match snapshot_contents {
            Some(expected) => {
                let on_disk = match fs::read(path) {
                    Ok(bytes) => match String::from_utf8(bytes) {
                        Ok(text) => Some(text),
                        Err(_) => {
                            return Err(CaptureError::io(format!(
                                "refusing to overwrite {}: note changed on disk after planning",
                                path.display()
                            ))
                            .with_typed(CaptureFailureKind::NoteChanged));
                        }
                    },
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        None
                    }
                    Err(error) => {
                        return Err(CaptureError::io(format!(
                            "failed to read {}: {error}",
                            path.display()
                        )));
                    }
                };
                if on_disk.as_deref() != Some(expected.as_str()) {
                    return Err(CaptureError::io(format!(
                        "refusing to overwrite {}: note changed on disk after planning",
                        path.display()
                    ))
                    .with_typed(CaptureFailureKind::NoteChanged));
                }
                let index = self.files.len();
                self.by_path.insert(path.to_path_buf(), index);
                self.files.push(BatchTextFile {
                    path: path.to_path_buf(),
                    existed: true,
                    present: true,
                    current: postimage,
                    original: expected,
                });
                Ok(())
            }
            None => {
                // No snapshot entry: the file did not exist when the
                // snapshot was built. If it exists now, that is a
                // concurrent creation; otherwise stage it as new.
                match fs::symlink_metadata(path) {
                    Ok(_) => Err(CaptureError::io(format!(
                        "refusing to overwrite {}: note was created on disk after planning",
                        path.display()
                    ))
                    .with_typed(CaptureFailureKind::NoteChanged)),
                    Err(error)
                        if error.kind() == io::ErrorKind::NotFound =>
                    {
                        let index = self.files.len();
                        self.by_path.insert(path.to_path_buf(), index);
                        self.files.push(BatchTextFile {
                            path: path.to_path_buf(),
                            existed: false,
                            present: true,
                            current: postimage.clone(),
                            original: String::new(),
                        });
                        Ok(())
                    }
                    Err(error) => Err(CaptureError::io(format!(
                        "failed to read {}: {error}",
                        path.display()
                    ))),
                }
            }
        }
    }

    pub(super) fn record_move(&mut self, source: &Path, destination: &Path) {
        self.move_edges
            .push((source.to_path_buf(), destination.to_path_buf()));
    }

    pub(super) fn moved_origin(
        &self,
        source: &Path,
        block_id: &str,
    ) -> Option<&MovedOrigin> {
        self.moved_origins
            .get(&(source.to_path_buf(), block_id.to_string()))
    }

    pub(super) fn record_moved_origin(
        &mut self,
        source: &Path,
        block_id: &str,
        destination: &Path,
        item_number: usize,
    ) {
        self.moved_origins.insert(
            (source.to_path_buf(), block_id.to_string()),
            MovedOrigin {
                destination: destination.to_path_buf(),
                item_number,
            },
        );
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

    pub(super) fn into_staged_files(
        self,
    ) -> Result<Vec<StagedTextFile>, CaptureError> {
        let CaptureBatchPlanner {
            files: staged,
            move_edges,
            ..
        } = self;
        let files: Vec<(usize, StagedTextFile)> = staged
            .into_iter()
            .enumerate()
            .filter(|(_, file)| {
                file.present && (!file.existed || file.current != file.original)
            })
            .map(|(index, file)| {
                (
                    index,
                    StagedTextFile {
                        target: file.path,
                        target_existed: file.existed,
                        original_target: file.original,
                        updated_target: file.current,
                    },
                )
            })
            .collect();
        let ordered = stable_move_write_order(&files, &move_edges)?;
        let mut files: Vec<Option<StagedTextFile>> =
            files.into_iter().map(|(_, file)| Some(file)).collect();
        Ok(ordered
            .into_iter()
            .map(|index| files[index].take().expect("ordered file exists"))
            .collect())
    }
}

fn stable_move_write_order(
    files: &[(usize, StagedTextFile)],
    move_edges: &[(PathBuf, PathBuf)],
) -> Result<Vec<usize>, CaptureError> {
    let by_path: HashMap<&Path, usize> = files
        .iter()
        .enumerate()
        .map(|(index, (_, file))| (file.target.as_path(), index))
        .collect();
    let mut outgoing = vec![BTreeSet::new(); files.len()];
    let mut indegree = vec![0usize; files.len()];
    for (source, destination) in move_edges {
        let (Some(&source), Some(&destination)) = (
            by_path.get(source.as_path()),
            by_path.get(destination.as_path()),
        ) else {
            continue;
        };
        if source != destination && outgoing[destination].insert(source) {
            indegree[source] += 1;
        }
    }

    // Ready nodes are ordered by original first-touch index. Newly freed
    // nodes re-enter at their original position, making this Kahn traversal
    // stable while honoring every destination-before-source edge.
    let mut ready = BTreeSet::new();
    for (node, degree) in indegree.iter().enumerate() {
        if *degree == 0 {
            ready.insert((files[node].0, node));
        }
    }
    let mut order = Vec::with_capacity(files.len());
    while let Some((_, node)) = ready.pop_first() {
        order.push(node);
        for next in outgoing[node].iter().copied() {
            indegree[next] -= 1;
            if indegree[next] == 0 {
                ready.insert((files[next].0, next));
            }
        }
    }
    if order.len() != files.len() {
        return Err(CaptureError::io(
            "capture move write ordering contains a cycle",
        ));
    }
    Ok(order)
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
            pomodoro_refs: Vec::new(),
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
        CaptureKind::Transfer { .. } => {
            return Err(CaptureError::io(
                "transfer capture invariant failed: execution was not rejected before planning",
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
        CaptureKind::PomodoroSplit { .. } => {
            return Err(CaptureError::io(
                "pomodoro split capture invariant failed: wrong write planner",
            ));
        }
        CaptureKind::PomodoroMerge { .. } => {
            return Err(CaptureError::io(
                "pomodoro merge capture invariant failed: wrong write planner",
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
        CaptureKind::PomodoroPast { .. } => {
            return Err(CaptureError::io(
                "pomodoro past capture invariant failed: wrong write planner",
            ));
        }
        CaptureKind::TaskComplete { .. } => {
            return Err(CaptureError::io(
                "task complete capture invariant failed: wrong write planner",
            ));
        }
        CaptureKind::Ref(_) => {
            return Err(CaptureError::io(
                "reference capture invariant failed: wrong write planner",
            ));
        }
        CaptureKind::NotePull { .. } => {
            return Err(CaptureError::io(
                "note-pull capture is planned by plan_note_pull_item",
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
        pomodoro_refs: Vec::new(),
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
    /// Pomodoro block touches the batch loop feeds to the block tracker.
    /// Empty for captures that never touch the day file; link-presence
    /// toggles, Pomodoro notes, and project notes stay empty and rely on
    /// auto-detection instead.
    pub(super) pomodoro_refs: Vec<PomodoroBlockRef>,
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
    pub(super) dropped_dependencies: Vec<DroppedDependencyJson>,
    pub(super) task_block_ref: Option<TaskBlockRef>,
    pub(super) blocks_updates: Vec<BlocksLineUpdateJson>,
}

#[derive(Debug)]
pub(super) struct SubBulletCaptureDetails {
    pub(super) block_id: Option<String>,
    pub(super) parent_line: usize,
    pub(super) parent_text: String,
    pub(super) drawer: Option<DrawerJson>,
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
    /// The typed `~<K>` list, ascending, omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) drop: Vec<u32>,
    /// Rows removed by `~<K>`, omitted when empty. Dropped rows stay out of
    /// `tasks`, so an older client's "N queued tasks" stays truthful.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) dropped: Vec<PomodoroStartTaskJson>,
    /// Override outcome, present only when a start restarted or swapped a
    /// running session. Idle starts omit it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) r#override: Option<PomodoroStartOverrideJson>,
    /// Deprecated f6 field, retained for one release train. Omitted when true.
    #[serde(skip_serializing_if = "is_true")]
    pub(super) tracked: bool,
    /// Whether this session is off the work clock. Omitted when false.
    #[serde(skip_serializing_if = "is_false")]
    pub(super) off_clock: bool,
    /// Shared inherited note when the via rule applies. Omitted otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) clock_note: Option<String>,
    /// The event run a MEET start followed or overrode, omitted when the
    /// session is not a MEET with open lineup Event Tasks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) event_run: Option<EventRunJson>,
    /// Non-MEET start that newly crosses an open Event Task. Omitted when
    /// the session is MEET or the span does not newly cross an event.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) event_conflict: Option<EventConflictJson>,
    /// Non-MEET start whose written start sits strictly inside an open
    /// Event Task's meeting. Omitted when the session is MEET or S is
    /// not inside a meeting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) event_in_progress: Option<EventInProgressJson>,
}

fn is_true(value: &bool) -> bool {
    *value
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Override outcome for a restart or swap. `demoted` is swaps only;
/// restarts omit it. `close_first` is the draft that closes the running
/// session first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroStartOverrideJson {
    pub(super) action: &'static str,
    pub(super) ledger: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) previous: Option<PomodoroStartPreviousJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) demoted: Option<PomodoroStartDemotedJson>,
    pub(super) close_first: String,
}

fn overdue_is_false(value: &bool) -> bool {
    !*value
}

/// The running session in the pre-image: name (omitted when unnamed),
/// 1-based line, and its `HHMM` range plus duration. `overdue` is true
/// when `now` is at or after the session's end (start on today plus
/// duration); omitted when false. `homes` is the derived R9 object for
/// unnamed sessions only; named sessions do no resolution and omit it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroStartPreviousJson {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pomodoro_name: Option<String>,
    pub(super) pomodoro_line: usize,
    pub(super) start: String,
    pub(super) end: String,
    pub(super) duration_minutes: u64,
    pub(super) time_range: String,
    #[serde(skip_serializing_if = "overdue_is_false")]
    pub(super) overdue: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) homes: Option<crate::native::pomodoro_homes::Homes>,
}

/// The swapped-out session in the post-image: swaps only. `homes` follows
/// the same unnamed-only rule as `previous`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroStartDemotedJson {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pomodoro_name: Option<String>,
    pub(super) pomodoro_line: usize,
    pub(super) entry_line: String,
    pub(super) task_links: usize,
    pub(super) has_notes: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) homes: Option<crate::native::pomodoro_homes::Homes>,
}

/// One queued Task Link row on a started Pomodoro, following the close
/// row's explicit-null convention: unresolved rows carry `None` fields and
/// a `warning` instead of failing the start. Kept rows hold their pre-image
/// lineup number (so a drop leaves gaps like 1, 3); dropped rows carry the
/// pre-image ledger line plus the removed nested-line count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroStartTaskJson {
    pub(super) index: u32,
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
    /// Non-blank descendant lines removed with a dropped link, omitted
    /// when 0 (always 0 on kept rows).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(super) nested_lines: u32,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
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
    /// Non-MEET `+N` that newly crosses an open Event Task. Omitted when
    /// the session is MEET, the operator is `-N`, or nothing new is crossed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) event_conflict: Option<EventConflictJson>,
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
    /// Non-MEET `++N`/`--N` that newly crosses an open Event Task.
    /// Omitted when the session is MEET or nothing new is crossed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) event_conflict: Option<EventConflictJson>,
}

/// One new planned entry created by a `=/` split, with its type pill and
/// moved-link count. `line` is one-based in the post-image day file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroSplitCreated {
    #[serde(rename = "type")]
    pub(super) type_ref: crate::native::pomodoro_themes::ThemeRef,
    pub(super) line: usize,
    pub(super) name: String,
    pub(super) moved_links: usize,
}

/// `=/` split report: the source entry before/after, its base pill, every
/// created entry in source-name order (including zero-link entries), and
/// the count of source Task Links left behind (including unresolved and
/// untyped ones; nested lines never inflate counts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroSplitSummary {
    pub(super) path: String,
    pub(super) entry_line: usize,
    pub(super) before: String,
    pub(super) after: String,
    pub(super) base: crate::native::pomodoro_themes::ThemeRef,
    pub(super) created: Vec<PomodoroSplitCreated>,
    pub(super) stayed_links: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PomodoroMergeAbsorbedRole {
    Named,
    Next,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PomodoroMergeSourceRole {
    Current,
    NextUp,
    Named,
}

/// One absorbed dest entry from a `=*` merge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroMergeAbsorbed {
    pub(super) name: String,
    pub(super) role: PomodoroMergeAbsorbedRole,
    pub(super) from_line: usize,
    pub(super) moved_lines: usize,
    pub(super) dropped_duplicates: usize,
}

/// `=*` merge report: the survivor before/after and every absorbed dest
/// in selector order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroMergeSummary {
    pub(super) path: String,
    pub(super) entry_line: usize,
    pub(super) before: String,
    pub(super) after: String,
    pub(super) source_role: PomodoroMergeSourceRole,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) explicit: Option<String>,
    pub(super) absorbed: Vec<PomodoroMergeAbsorbed>,
    /// Task Links actually moved onto the survivor; used by human output.
    #[serde(skip)]
    pub(super) moved_links: usize,
}

#[derive(Debug)]
pub(super) struct PomodoroCaptureDetails {
    pub(super) block_id: String,
    pub(super) day_file: String,
    pub(super) block_link: String,
    pub(super) pomodoro_link_placement: Placement,
    /// Canonical destination name for `<text> @route:id[#NAME]`
    /// Pomodoro-task captures (`None` for legacy callers).
    pub(super) pomodoro_name: Option<String>,
    /// True when the capture created the destination entry.
    pub(super) creates_pomodoro: bool,
    /// Post-image destination endpoint (with the plan-budget `role`).
    pub(super) pomodoro_link_destination: Option<PomodoroLinkEndpoint>,
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
    Link,
    Unlink,
}

impl TaskToggleDirection {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Next => "next",
            Self::Link => "link",
            Self::Unlink => "unlink",
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
    pub(super) dropped_dependencies: Vec<DroppedDependencyJson>,
    pub(super) task_block_ref: Option<TaskBlockRef>,
    pub(super) blocks_updates: Vec<BlocksLineUpdateJson>,
    /// Item-level relation successor plan for an `@route+id!` unlink of a
    /// Pending task. Empty on every other toggle path.
    pub(super) relation: ToggleRelationPlan,
}

/// Relation-successor rows raised by a Pending unlink (`docs/task-dependencies.md`
/// §12.18 SA31). Default is silence.
#[derive(Debug, Default)]
pub(super) struct ToggleRelationPlan {
    pub(super) successors: Vec<TaskCompleteUnblockedJson>,
    pub(super) held: Vec<RelationHeldJson>,
    pub(super) check: Option<&'static str>,
    pub(super) groups: Vec<crate::native::task_complete::RelationOfferGroup>,
    pub(super) task_block_refs: Vec<TaskBlockRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroLinkEndpoint {
    pub(super) line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) time_range: Option<String>,
    /// How the destination entry was chosen: `current` (the running
    /// timed entry), `next_up` (implicitly chosen and not running),
    /// `named` (an existing entry matched by `#NAME`), or `created`
    /// (a new entry). Sources leave this unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) role: Option<&'static str>,
    /// The task's effective type when Bob placed the link by type (D7):
    /// the theme ref, the cascade source, and the human label. Set only
    /// on destinations Bob placed by type; legacy and named destinations
    /// leave this unset.
    #[serde(skip_serializing_if = "Option::is_none", rename = "type")]
    pub(super) placed_type: Option<PlacedType>,
    /// A merged-Pomodoro landing (R2): the destination gained the task's
    /// type. Additive: omitted when the landing was consistent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) merged: Option<MergedPomodoro>,
}

/// Additive R2 merge record on a link destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct MergedPomodoro {
    pub(super) before: String,
    pub(super) after: String,
    pub(super) added: Vec<super::super::pomodoro_themes::ThemeRef>,
}

/// Additive D7 typed-placement record on a link destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PlacedType {
    pub(super) theme: super::super::pomodoro_themes::ThemeRef,
    pub(super) source: super::super::pomodoro_themes::suggest::SuggestionSource,
    pub(super) label: String,
}

/// Resolve a link-destination role for the plan-budget contract: a
/// created entry is `created`, an explicit `#NAME` match is `named`,
/// otherwise a timed destination is the running `current` entry and
/// an untimed one is `next_up`.
pub(super) fn destination_role(
    creates_pomodoro: bool,
    selector: Option<&str>,
    time_range: Option<&str>,
) -> &'static str {
    if creates_pomodoro {
        "created"
    } else if selector.is_some() {
        "named"
    } else if time_range.is_some() {
        "current"
    } else {
        "next_up"
    }
}

#[cfg(test)]
mod move_order_tests {
    use super::*;
    use std::fs;

    #[test]
    fn replacement_order_is_stable_and_destinations_precede_sources() {
        let dir = tempfile::tempdir().expect("temp vault");
        let source = dir.path().join("cash.md");
        let middle = dir.path().join("cash_x.md");
        let unrelated = dir.path().join("repair.md");
        let destination = dir.path().join("cash_x_y.md");
        for path in [&source, &middle, &unrelated, &destination] {
            fs::write(path, "before\n").expect("write note");
        }
        let mut planner = CaptureBatchPlanner::default();
        for path in [&source, &middle, &unrelated, &destination] {
            planner
                .stage(path, "after\n".to_string())
                .expect("stage note");
        }
        planner.record_move(&source, &middle);
        planner.record_move(&middle, &destination);

        let order = planner
            .into_staged_files()
            .expect("topologically order staged files")
            .into_iter()
            .map(|file| {
                file.target
                    .file_name()
                    .expect("filename")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(order, ["repair.md", "cash_x_y.md", "cash_x.md", "cash.md"]);
    }

    #[test]
    fn cyclic_move_order_refuses_before_returning_staged_files() {
        let dir = tempfile::tempdir().expect("temp vault");
        let first = dir.path().join("first.md");
        let second = dir.path().join("second.md");
        for path in [&first, &second] {
            fs::write(path, "before\n").expect("write note");
        }
        let mut planner = CaptureBatchPlanner::default();
        planner
            .stage(&first, "after\n".to_string())
            .expect("stage first");
        planner
            .stage(&second, "after\n".to_string())
            .expect("stage second");
        planner.record_move(&first, &second);
        planner.record_move(&second, &first);

        let error = planner
            .into_staged_files()
            .expect_err("cyclic move ordering refuses");
        assert!(error.message.contains("ordering contains a cycle"));
    }
}

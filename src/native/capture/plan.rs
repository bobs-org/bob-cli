//! Batch/item planning and capture formatting helpers.
use super::*;

pub(super) struct PlannedCaptureBatch {
    pub(super) items: Vec<PlannedCaptureItem>,
    pub(super) text_files: Vec<StagedTextFile>,
    pub(super) global_destination: Option<GlobalDestinationSummary>,
    pub(super) warnings: Vec<CaptureWarning>,
    pub(super) plan_budget: Option<CapturePlanBudget>,
    pub(super) pomodoro_blocks: Vec<PomodoroBlockJson>,
    pub(super) task_blocks: Vec<TaskBlockJson>,
    pub(super) inbox_route: InboxRouteReport,
    /// Routed tasks' source→destination landings, in move order. The
    /// block trackers and `open_notes` forward entries through these.
    pub(super) routed_landings: Vec<RoutedTaskMove>,
    pub(super) template: Option<capture_language::template::TemplateSummary>,
    pub(super) template_rows: Vec<Option<capture_language::TemplateRowSummary>>,
    /// Ref jobs staged by reference items, in batch order. The commit
    /// enqueues them as rolled-back side effects and patches each
    /// item's `ref.job` with the created id.
    pub(super) staged_jobs: Vec<StagedRefJob>,
}

/// One reference item's staged ref job plus its position in the
/// batch's `items`, so the commit can patch the created job id back.
pub(super) struct StagedRefJob {
    pub(super) item: usize,
    pub(super) job: ref_jobs::NewJob,
}

pub(super) struct PlannedCaptureItem {
    pub(super) result: CaptureItemResult,
    pub(super) clip_plan: Option<capture_clip::ClipPlan>,
    /// Pomodoro block touches this item reported. The batch loop feeds
    /// them to the block tracker; this field is never serialized.
    pub(super) pomodoro_refs: Vec<PomodoroBlockRef>,
    /// Parent-task touches this item reported. The batch loop feeds them
    /// to the task tracker; this field is never serialized.
    pub(super) task_block_refs: Vec<TaskBlockRef>,
}

pub(super) fn plan_capture_batch(
    request: &CaptureRequest,
    routing: Option<&UrlRoutingPolicy>,
) -> Result<PlannedCaptureBatch, CaptureError> {
    plan_capture_batch_with_config_path(request, routing, None)
}

pub(super) fn plan_capture_batch_with_config_path(
    request: &CaptureRequest,
    routing: Option<&UrlRoutingPolicy>,
    config_path: Option<&Path>,
) -> Result<PlannedCaptureBatch, CaptureError> {
    let parse_clip_markers = request.forced_clip.is_none() && !request.no_clip;
    // A forced `-r` route is an allowed explicit ref parent; every other
    // forced destination or clip flag keeps the item a task.
    let options = CaptureParseOptions {
        parse_clip_markers,
        url_routing: routing,
        explicit_destination: request.forced_section.is_some()
            || request.forced_sub_bullet_target.is_some()
            || request.forced_task_section.is_some()
            || request.forced_clip.is_some(),
        has_global_destination: false,
    };
    let parsed_draft = parse_capture_draft_with_clip_control(
        &request.raw_text,
        request.forced_route.as_deref(),
        request.forced_section.as_deref(),
        &options,
    )?;
    if parsed_draft.template.is_some() && request.forced_clip.is_some() {
        return Err(CaptureError::usage_code(
            "a template can't capture the clipboard: `%` would repeat once per row",
            "template_clipboard",
        ));
    }
    if parsed_draft.global.is_some()
        && !request.forced_destination_flags.is_empty()
    {
        return Err(CaptureError::usage(competing_destination_error(
            &request.forced_destination_flags,
        )));
    }
    let template = parsed_draft.template;
    let template_rows = parsed_draft
        .items
        .iter()
        .map(|item| item.template_row.clone())
        .collect();
    let parsed_items = parsed_draft.items;
    let mut warnings: Vec<CaptureWarning> = parsed_draft
        .warnings
        .into_iter()
        .map(CaptureWarning::Text)
        .collect();
    let global_destination =
        parsed_draft.global.map(|global| GlobalDestinationSummary {
            mode: global.mode_label(),
            route: global.route,
            block_id: global.block_id,
        });
    let now = bob_env::current_datetime();
    let today = now.date();
    let roll_seed = config::roll_seed();
    let mut planner = CaptureBatchPlanner::default();
    let mut clip_reservations = capture_clip::ClipReservations::default();
    let mut items = Vec::new();
    let day_file = pomodoro::day_file_for(&request.bob_dir);
    let day_relative =
        capture_pomodoros::relative_day_file(&day_file, &request.bob_dir);
    let mut block_tracker = PomodoroBlockTracker::new(day_relative);
    let task_settings = note_tasks::read_settings(&request.bob_dir);
    let mut task_tracker = TaskBlockTracker::new(task_settings.clone());
    let mut dependency_ctx = DependencyContext::new(&request.bob_dir, today);
    let mut task_theme_context =
        CaptureTaskThemeContext::new(&request.bob_dir, today, config_path);
    let mut ref_assignments = assign_ref_verdicts(request, &parsed_items);
    let mut staged_jobs = Vec::new();

    for parsed_item in parsed_items {
        let item_number = parsed_item.index + 1;
        let error_context = capture_error_context(item_number, &parsed_item);
        let pre_day = planner.peek_text(&day_file);
        planner.begin_item(pre_day.clone());
        let pre_tasks = task_tracker.snapshot_pre(&planner);
        if matches!(parsed_item.parsed.kind, CaptureKind::Ref(_)) {
            let assignment = ref_assignments
                .remove(&parsed_item.index)
                .ok_or_else(|| {
                    CaptureError::io(
                        "reference capture invariant failed: missing verdict",
                    )
                })?;
            let (mut planned, staged) =
                plan_ref_item(request, parsed_item, assignment, today)
                    .map_err(|mut error| {
                        error.message =
                            format!("{error_context}: {}", error.message);
                        error
                    })?;
            planned.result.consumed_links = planner.take_consumed_links();
            if let Some(job) = staged {
                staged_jobs.push(StagedRefJob {
                    item: items.len(),
                    job,
                });
            }
            items.push(planned);
            continue;
        }
        let mut planned = plan_capture_item(
            request,
            parsed_item,
            now,
            today,
            roll_seed,
            &mut planner,
            &mut clip_reservations,
            &mut warnings,
            &mut dependency_ctx,
            config_path,
            &mut task_theme_context,
        )
        .map_err(|mut error| {
            error.message = format!("{error_context}: {}", error.message);
            error
        })?;
        planned.result.consumed_links = planner.take_consumed_links();
        planned.result.normalize_dependency_updates();
        let refs = std::mem::take(&mut planned.pomodoro_refs);
        match planner.loaded_texts(&day_file) {
            Some((original, current)) => {
                let pre = pre_day.unwrap_or(original);
                block_tracker.track_item(Some(&pre), Some(&current), refs);
            }
            None => {
                debug_assert!(
                    refs.is_empty(),
                    "pomodoro block refs without a loaded day file"
                );
            }
        }
        let task_refs = std::mem::take(&mut planned.task_block_refs);
        task_tracker.track_item(&planner, &pre_tasks, task_refs);
        items.push(planned);
    }

    // Net successor reporting (SL20): drop linked rows whose bullet or
    // open status a later item took back, before blocks resolve reasons.
    apply_successor_net_post_pass(
        &mut items,
        &mut planner,
        &dependency_ctx,
        &request.bob_dir,
    );
    let inbox_route = detect_inbox_routes(
        &planner,
        &request.bob_dir,
        &task_settings,
        &dependency_ctx,
    )?;
    // Answered route moves run here: after every item, the successor
    // net post-pass, and candidate detection, but before the block
    // trackers finish and the staged files freeze.
    let applied = apply_inbox_routes(
        &mut planner,
        &request.bob_dir,
        &task_settings,
        today,
        inbox_route,
        &request.inbox_routes,
    )?;
    task_tracker.remap_routed_tasks(&applied.moves);
    let final_day = planner.peek_text(&day_file);
    let successor_links = surviving_successor_links(&items);
    let pomodoro_blocks = block_tracker
        .finish_with_unblocked(final_day.as_deref(), &successor_links);
    let task_blocks = task_tracker.finish(&planner);
    // The BLOCKS kill-switch warning surfaces once per batch, and only
    // when a dependency write consulted the switch (§14.10). Plain
    // captures never touch the config and never warn.
    if let Some(warning) = dependency_ctx.blocks_lines_warning()
        && !warnings
            .iter()
            .any(|known| known.as_text() == Some(warning.as_str()))
    {
        warnings.push(warning.into());
    }
    if let Some(warning) = dependency_ctx.linked_from_lines_warning()
        && !warnings
            .iter()
            .any(|known| known.as_text() == Some(warning.as_str()))
    {
        warnings.push(warning.into());
    }
    // The close-label policy warning (§17 CL2 / §18 CP2) surfaces once
    // per batch, and only when a relation gesture consulted the policy.
    // Plain captures never touch the config and never warn.
    if let Some(warning) = dependency_ctx.cancel_labels_warning()
        && !warnings
            .iter()
            .any(|known| known.as_text() == Some(warning.as_str()))
    {
        warnings.push(warning.into());
    }
    Ok(PlannedCaptureBatch {
        items,
        text_files: planner.into_staged_files(),
        global_destination,
        warnings,
        plan_budget: None,
        pomodoro_blocks,
        task_blocks,
        inbox_route: applied.report,
        routed_landings: applied.moves,
        template,
        template_rows,
        staged_jobs,
    })
}

/// One reference item's verdict plus the display fields its result
/// and staged job need.
struct RefAssignment {
    intent: RefIntent,
    verdict: &'static str,
    path: Option<String>,
    title: Option<String>,
    reading_state: Option<String>,
    message: Option<String>,
}

/// Offline verdicts for every reference item in the draft, keyed by
/// item index. Library verdicts and the spool's pending keys are each
/// read once per batch. A `clipping` spool hit and a `duplicate`
/// repeat within the draft both win over the library verdict.
fn assign_ref_verdicts(
    request: &CaptureRequest,
    parsed_items: &[ParsedCaptureItem],
) -> HashMap<usize, RefAssignment> {
    let mut positions = Vec::new();
    for parsed_item in parsed_items {
        if let CaptureKind::Ref(intent) = &parsed_item.parsed.kind {
            positions.push((parsed_item.index, intent));
        }
    }
    if positions.is_empty() {
        return HashMap::new();
    }
    let intents: Vec<&UrlIntent> =
        positions.iter().map(|(_, intent)| &intent.intent).collect();
    let library = url_routing::library_verdicts(&request.bob_dir, &intents);
    let spool_keys = ref_jobs::pending_keys(&ref_jobs::jobs_dir());
    let mut seen: HashMap<&str, usize> = HashMap::new();
    let mut assignments = HashMap::new();
    for ((index, intent), verdict) in positions.into_iter().zip(library) {
        let item_number = index + 1;
        let assignment =
            if let Some(first) = seen.get(intent.dedupe_key.as_str()) {
                RefAssignment {
                    intent: (*intent).clone(),
                    verdict: "duplicate",
                    path: None,
                    title: None,
                    reading_state: None,
                    message: Some(format!("same link as item {first}")),
                }
            } else if spool_keys.contains(intent.dedupe_key.as_str()) {
                RefAssignment {
                    intent: (*intent).clone(),
                    verdict: "clipping",
                    path: None,
                    title: None,
                    reading_state: None,
                    message: None,
                }
            } else {
                RefAssignment {
                    intent: (*intent).clone(),
                    verdict: verdict.verdict.as_str(),
                    path: verdict.path,
                    title: verdict.title,
                    reading_state: verdict.reading_state,
                    message: verdict.message,
                }
            };
        seen.entry(intent.dedupe_key.as_str())
            .or_insert(item_number);
        assignments.insert(index, assignment);
    }
    assignments
}

fn capture_error_context(
    item_number: usize,
    parsed_item: &ParsedCaptureItem,
) -> String {
    match &parsed_item.template_row {
        Some(row) => format!("template row {} (line {})", row.row, row.line),
        None => format!(
            "capture item {item_number} starting on line {}",
            parsed_item.line_start
        ),
    }
}

/// Plan one reference item: resolve the canonical parent with
/// `parent_notes::resolve_parent` before staging anything, then report an
/// honest result with no vault write, plus a staged ref job when the link
/// still needs clipping. Queued items preview the exact parent task line
/// the worker's fallback writes, so the fallback matches it byte for byte.
/// An unresolvable route fails the item with the resolver's hints and the
/// whole batch rolls back.
fn shell_quote_listen_url(url: &str) -> String {
    let bare = !url.is_empty()
        && url.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'.' | b'_'
                        | b'~'
                        | b':'
                        | b'/'
                        | b'%'
                        | b'+'
                        | b'='
                        | b'-'
                )
        });
    if bare {
        url.to_string()
    } else {
        format!("'{}'", url.replace('\'', "'\\''"))
    }
}

fn listen_hint_for_unchanged(cleaned_url: &str, parent_route: &str) -> String {
    format!(
        "no new listen job was queued · to attach audio run: bob ref create {} -P {parent_route} -L",
        shell_quote_listen_url(cleaned_url),
    )
}

fn plan_ref_item(
    request: &CaptureRequest,
    parsed_item: ParsedCaptureItem,
    assignment: RefAssignment,
    today: NaiveDate,
) -> Result<(PlannedCaptureItem, Option<ref_jobs::NewJob>), CaptureError> {
    let intent = assignment.intent;
    let queued =
        matches!(assignment.verdict, "not_found" | "legacy" | "unknown");
    let (relative_target, target) = match assignment.verdict {
        "in_library" | "in_intake" => match assignment.path.clone() {
            Some(relative) => (
                relative.clone(),
                request.bob_dir.join(&relative).display().to_string(),
            ),
            None => (String::new(), String::new()),
        },
        _ => (String::new(), String::new()),
    };
    // Lexical token plus source: an inline `@route` or forced `-r` is
    // explicit, a plain `@@route` upgrade is global, otherwise the default
    // inbox. The planning layer resolves the token to its canonical route.
    let token = parsed_item
        .parsed
        .route
        .clone()
        .unwrap_or_else(|| inbox_route().to_string());
    let source = parsed_item
        .ref_source
        .unwrap_or(crate::native::capture_language::RefParentSource::Default);
    let resolved =
        crate::native::parent_notes::resolve_parent(&request.bob_dir, &token)
            .map_err(|error| CaptureError::usage(error.message()))?;
    let parent_route = resolved.route.clone();
    let parent_label = resolved.label.clone();
    let parent_kind = resolved.kind_name();
    let parent_alias = match &resolved.matched {
        crate::native::parent_notes::ParentMatchKind::Alias(alias) => {
            Some(alias.clone())
        }
        crate::native::parent_notes::ParentMatchKind::Stem => None,
    };
    let parent_json = RefParentJson {
        route: parent_route.clone(),
        label: parent_label,
        kind: parent_kind,
        source: source.as_str(),
        alias: parent_alias,
    };
    let created = date_string(today);
    // The fallback task line holds only the URL body in the resolved
    // parent note, so the worker's fallback matches it byte for byte.
    let fallback_target = format!("{parent_route}.md");
    let fallback_task_line =
        format_task_line(&parsed_item.parsed.body, &created, None, None);
    let fallback = queued.then(|| RefFallbackJson {
        relative_target: fallback_target.clone(),
        task_line: fallback_task_line.clone(),
    });
    let listen = intent.listen;
    let staged = queued.then(|| ref_jobs::NewJob {
        source: "capture".to_string(),
        bob_dir: request.bob_dir.clone(),
        url: intent.original.clone(),
        cleaned_url: intent.cleaned.clone(),
        dedupe_key: intent.dedupe_key.clone(),
        display: intent.display.clone(),
        route_hint: intent.route_hint.as_str().to_string(),
        listen,
        parent: Some(parent_route.clone()),
        fallback: ref_jobs::JobFallback {
            relative_target: fallback_target,
            task_line: fallback_task_line,
        },
    });
    // Unlike an explicit `ref create -L`, the shorthand never adds audio
    // to existing captures: a marked but unchanged item queues nothing
    // and carries the explicit retry command instead.
    let listen_hint = (!queued && listen)
        .then(|| listen_hint_for_unchanged(&intent.cleaned, &parent_route));
    let planned = PlannedCaptureItem {
        result: CaptureItemResult {
            task_theme: None,
            ok: true,
            dry_run: request.dry_run,
            routed: false,
            route: None,
            route_label: String::new(),
            relative_target,
            target,
            text: intent.original.clone(),
            task_line: String::new(),
            kind: capture_kind_label(&parsed_item.parsed.kind),
            created,
            scheduled: None,
            priority: None,
            priority_label: None,
            placement: if queued {
                Placement::Queued
            } else {
                Placement::Unchanged
            },
            sub_bullets: Vec::new(),
            clip: None,
            schedule_log: None,
            block_id: None,
            day_file: None,
            block_link: None,
            pomodoro_link_placement: None,
            parent_line: None,
            parent_text: None,
            parent_section: None,
            parent_status_symbol: None,
            parent_status_name: None,
            toggle_direction: None,
            previous_task_line: None,
            status_symbol: None,
            status_name: None,
            previous_status_symbol: None,
            previous_status_name: None,
            pomodoro_name: None,
            creates_pomodoro: None,
            pomodoro_already_linked: None,
            removed_pomodoro_links: None,
            removed_scheduled: None,
            pomodoro_selector_unused: None,
            toggle_behavior: None,
            status_changed: None,
            pomodoro_link_action: None,
            pomodoro_link_source: None,
            pomodoro_link_destination: None,
            project_note: None,
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_close: None,
            pomodoro_reset: None,
            dependency_update: None,
            consumed_links: Vec::new(),
            task_complete: None,
            r#ref: Some(RefItemJson {
                url: intent.original.clone(),
                cleaned_url: intent.cleaned.clone(),
                dedupe_key: intent.dedupe_key.clone(),
                display: intent.display.clone(),
                route_hint: intent.route_hint.as_str(),
                listen,
                library: RefLibraryJson {
                    verdict: assignment.verdict,
                    path: assignment.path,
                    title: assignment.title,
                    reading_state: assignment.reading_state,
                    message: assignment.message,
                },
                parent: parent_json,
                job: None,
                fallback,
                listen_hint,
            }),
            toggle_task_description: None,
            link_guard: None,
            dropped_dependencies: Vec::new(),
            blocks_line_updates: Vec::new(),
            minted_dependent_block_id: None,
            dependency_updates: Vec::new(),
            minted_blocker_block_id: None,
            relation_updates: Vec::new(),
            relation_cancels: Vec::new(),
            relation_completes: Vec::new(),
            linked_from_line_updates: Vec::new(),
            minted_relation_block_ids: Vec::new(),
            mirror_skipped: Vec::new(),
            relation_label_hides: Vec::new(),
        },
        clip_plan: None,
        pomodoro_refs: Vec::new(),
        task_block_refs: Vec::new(),
    };
    Ok((planned, staged))
}

pub(super) fn plan_capture_item(
    request: &CaptureRequest,
    parsed_item: ParsedCaptureItem,
    now: chrono::NaiveDateTime,
    today: NaiveDate,
    roll_seed: u64,
    planner: &mut CaptureBatchPlanner,
    clip_reservations: &mut capture_clip::ClipReservations,
    warnings: &mut Vec<CaptureWarning>,
    dependency_ctx: &mut DependencyContext,
    config_path: Option<&Path>,
    task_theme_context: &mut CaptureTaskThemeContext,
) -> Result<PlannedCaptureItem, CaptureError> {
    let mut parsed = parsed_item.parsed;
    // A dependency-only capture updates an explicitly selected existing
    // task: no new task, empty child, link toggle, or Pomodoro action.
    // It never flows through the toggle/link planners below. Inherited
    // `@@route+id` items land as empty SubBullet owners and take the
    // same path so they write the edge without an empty bullet.
    if let Some(target) = relationship_only_existing_owner(&parsed) {
        return plan_dependency_only_item(
            request,
            &parsed,
            &target,
            planner,
            warnings,
            dependency_ctx,
        );
    }
    if let CaptureKind::TaskComplete {
        raw,
        note,
        block_id,
        ..
    } = parsed.kind.clone()
    {
        return plan_task_complete_item(
            request,
            &parsed,
            &raw,
            &note,
            &block_id,
            today,
            planner,
            dependency_ctx,
        );
    }
    if let CaptureKind::PomodoroClose { spec } = parsed.kind.clone() {
        if !parsed.blocked_tasks.is_empty() {
            return Err(CaptureError::usage(
                capture_language::close_blocked_task_error(&close_star_marker(
                    &parsed,
                )),
            ));
        }
        return plan_pomodoro_close_item(
            request,
            parsed,
            spec,
            now,
            today,
            planner,
            warnings,
            dependency_ctx,
        );
    }
    if let CaptureKind::PomodoroStart {
        spec,
        pomodoro_name,
        pomodoro_name_is_literal,
    } = parsed.kind.clone()
    {
        return plan_pomodoro_start_item(
            request,
            parsed,
            spec,
            pomodoro_name,
            pomodoro_name_is_literal,
            now,
            today,
            planner,
            warnings,
        );
    }
    if let CaptureKind::PomodoroAdjust { spec } = parsed.kind.clone() {
        return plan_pomodoro_adjust_item(
            request, parsed, spec, today, planner,
        );
    }
    if let CaptureKind::PomodoroShift { spec } = parsed.kind.clone() {
        return plan_pomodoro_shift_item(request, parsed, spec, today, planner);
    }
    if let Some(target) = request.forced_sub_bullet_target.as_ref() {
        parsed.kind = CaptureKind::SubBullet {
            target: target.clone(),
            section: None,
        };
        // A forced sub-bullet parent owns the item's modifiers like an
        // explicit marker does; a picker-ref target cannot.
        if !parsed.dependencies.is_empty()
            || !parsed.blocked_tasks.is_empty()
            || !parsed.relations.is_empty()
        {
            match target {
                SubBulletTarget::BlockId(block_id) => {
                    let Some(route) = parsed.route.clone() else {
                        return Err(CaptureError::usage(format!(
                            "{} with --task need --route for the dependent note",
                            typed_relationship_phrase(&parsed),
                        )));
                    };
                    parsed.dependency_target = Some(ParsedDependencyTarget {
                        kind: ParsedDependencyTargetKind::ExistingTask,
                        route: Some(route),
                        block_id: Some(block_id.clone()),
                        inherited: false,
                    });
                }
                SubBulletTarget::Ref { .. } => {
                    return Err(CaptureError::usage(format!(
                        "{} cannot attach to a --task-ref picker selection",
                        typed_relationship_phrase(&parsed),
                    )));
                }
            }
        }
    }
    if let Some(title) = request.forced_task_section.as_ref() {
        match &mut parsed.kind {
            CaptureKind::SubBullet { section, .. } => {
                *section = Some(TaskSectionSelector {
                    text: title.clone(),
                    exact: true,
                });
            }
            _ => {
                return Err(CaptureError::usage(
                    "--task-section requires --task or --task-ref",
                ));
            }
        }
    }
    if let Some(clip) = request.forced_clip.as_ref() {
        parsed.clip = Some(clip.clone());
    }
    // Forced `--task`/`--route` rewrites the kind to SubBullet after
    // the first relationship-only check; empty-body owners still skip
    // the empty-bullet path.
    if let Some(target) = relationship_only_existing_owner(&parsed) {
        return plan_dependency_only_item(
            request,
            &parsed,
            &target,
            planner,
            warnings,
            dependency_ctx,
        );
    }
    if let CaptureKind::TaskToggle {
        block_id,
        pomodoro_name,
        pomodoro_name_is_literal,
        intent,
    } = &parsed.kind
    {
        reject_task_toggle_conflicts(&parsed, request)?;
        let route = parsed.route.as_deref().ok_or_else(|| {
            CaptureError::io(
                "task toggle capture invariant failed: route is missing",
            )
        })?;
        let created = date_string(today);
        let relative_target = relative_target(Some(route));
        let target = request.bob_dir.join(&relative_target);
        let (mut note_plan, link_guard) = plan_task_toggle_capture(
            planner,
            &request.bob_dir,
            &target,
            route,
            block_id,
            pomodoro_name.as_deref(),
            *pomodoro_name_is_literal,
            *intent,
            today,
            warnings,
            dependency_ctx,
        )?;
        let pomodoro_refs = std::mem::take(&mut note_plan.pomodoro_refs);
        let task_theme = if link_guard.is_none()
            && *intent == TaskToggleIntent::EnsureNext
            && pomodoro_name.is_some()
        {
            let toggle = note_plan.toggle.as_ref().ok_or_else(|| {
                CaptureError::io(
                    "task toggle capture invariant failed: missing toggle details",
                )
            })?;
            task_theme_context
                .apply_existing_task(
                    planner,
                    &target,
                    route,
                    block_id,
                    &toggle.previous_task_line,
                    toggle.pomodoro_name.as_deref(),
                    warnings,
                )?
                .map(|applied| {
                    let task_line = applied.task_line.clone();
                    note_plan
                        .toggle
                        .as_mut()
                        .expect("toggle details were checked above")
                        .task_line = task_line;
                    applied.summary
                })
        } else {
            None
        };
        let toggle = note_plan.toggle.as_ref().ok_or_else(|| {
            CaptureError::io(
                "task toggle capture invariant failed: missing toggle details",
            )
        })?;
        let link_consumed = link_guard.is_some();
        return Ok(PlannedCaptureItem {
            result: CaptureItemResult {
                ok: true,
                dry_run: request.dry_run,
                routed: true,
                route: Some(route.to_string()),
                route_label: route_label(route),
                relative_target: relative_target.to_string_lossy().into_owned(),
                target: target.display().to_string(),
                text: String::new(),
                task_line: toggle.task_line.clone(),
                task_theme,
                kind: capture_kind_label(&parsed.kind),
                created,
                scheduled: None,
                priority: None,
                priority_label: None,
                placement: note_plan.placement,
                sub_bullets: Vec::new(),
                clip: None,
                schedule_log: toggle.schedule_log.clone(),
                block_id: Some(toggle.block_id.clone()),
                day_file: (!link_consumed || toggle.removed_pomodoro_links > 0)
                    .then(|| toggle.day_file.clone()),
                block_link: (!link_consumed).then(|| toggle.block_link.clone()),
                pomodoro_link_placement: toggle.pomodoro_link_placement,
                parent_line: None,
                parent_text: None,
                parent_section: None,
                parent_status_symbol: None,
                parent_status_name: None,
                toggle_direction: (!link_consumed)
                    .then(|| toggle.direction.label()),
                previous_task_line: Some(toggle.previous_task_line.clone()),
                status_symbol: Some(toggle.status_symbol),
                status_name: Some(toggle.status_name.clone()),
                previous_status_symbol: Some(toggle.previous_status_symbol),
                previous_status_name: Some(toggle.previous_status_name.clone()),
                pomodoro_name: toggle.pomodoro_name.clone(),
                creates_pomodoro: (!link_consumed)
                    .then_some(toggle.creates_pomodoro),
                pomodoro_already_linked: (!link_consumed)
                    .then_some(toggle.pomodoro_already_linked),
                removed_pomodoro_links: Some(toggle.removed_pomodoro_links),
                removed_scheduled: toggle.removed_scheduled.clone(),
                pomodoro_selector_unused: (!link_consumed)
                    .then_some(toggle.pomodoro_selector_unused),
                toggle_behavior: toggle.toggle_behavior,
                status_changed: toggle.status_changed,
                pomodoro_link_action: toggle.pomodoro_link_action,
                pomodoro_link_source: toggle.pomodoro_link_source.clone(),
                pomodoro_link_destination: toggle
                    .pomodoro_link_destination
                    .clone(),
                project_note: None,
                pomodoro_start: None,
                pomodoro_adjust: None,
                pomodoro_shift: None,
                pomodoro_close: None,
                pomodoro_reset: None,
                dependency_update: None,
                consumed_links: Vec::new(),
                task_complete: None,
                toggle_task_description: Some(toggle.task_description.clone()),
                r#ref: None,
                link_guard,
                dropped_dependencies: toggle.dropped_dependencies.clone(),
                blocks_line_updates: toggle.blocks_updates.clone(),
                minted_dependent_block_id: None,
                dependency_updates: Vec::new(),
                minted_blocker_block_id: None,
                relation_updates: Vec::new(),
                relation_cancels: Vec::new(),
                relation_completes: Vec::new(),
                linked_from_line_updates: Vec::new(),
                minted_relation_block_ids: Vec::new(),
                mirror_skipped: Vec::new(),
                relation_label_hides: Vec::new(),
            },
            clip_plan: None,
            pomodoro_refs,
            task_block_refs: toggle
                .task_block_ref
                .clone()
                .into_iter()
                .collect(),
        });
    }
    if let CaptureKind::PomodoroLink {
        block_id,
        pomodoro_name,
        pomodoro_name_is_literal,
        start,
        close,
        ..
    } = &parsed.kind.clone()
    {
        if let Some(close_spec) = close.clone() {
            return plan_pomodoro_close_link_item(
                request,
                parsed,
                now,
                today,
                planner,
                warnings,
                block_id,
                close_spec,
                dependency_ctx,
            );
        }
        reject_pomodoro_link_conflicts(&parsed, request)?;
        let route = parsed.route.as_deref().ok_or_else(|| {
            CaptureError::io(
                "pomodoro link capture invariant failed: route is missing",
            )
        })?;
        let target = request.bob_dir.join(relative_target(Some(route)));
        let ledger_target = match pomodoro_name.as_deref() {
            Some(name) if *pomodoro_name_is_literal => {
                capture_task_toggle::PomodoroLinkTarget::LiteralName(name)
            }
            Some(name) => capture_task_toggle::PomodoroLinkTarget::Named(name),
            None => capture_task_toggle::PomodoroLinkTarget::Implicit,
        };
        let mut planned = plan_existing_task_pomodoro_link_item(
            planner,
            &request.bob_dir,
            request.dry_run,
            &target,
            route,
            block_id,
            ledger_target,
            start.as_ref(),
            now,
            today,
            warnings,
            dependency_ctx,
        )?;
        // This capture-only boundary keeps `bob ref today` on the shared
        // link planner's explicit never-deliberate path.
        if pomodoro_name.is_some() && planned.result.link_guard.is_none() {
            let previous_task_line = planned
                .result
                .previous_task_line
                .as_deref()
                .ok_or_else(|| {
                    CaptureError::io(
                        "pomodoro link capture invariant failed: missing previous task line",
                    )
                })?;
            let resolved_name = planned
                .result
                .pomodoro_link_destination
                .as_ref()
                .and_then(|destination| destination.name.as_deref());
            if let Some(applied) = task_theme_context.apply_existing_task(
                planner,
                &target,
                route,
                block_id,
                previous_task_line,
                resolved_name,
                warnings,
            )? {
                planned.result.task_line = applied.task_line;
                planned.result.task_theme = Some(applied.summary);
            }
        }
        return Ok(planned);
    }
    if matches!(parsed.kind, CaptureKind::ProjectNote { .. }) {
        return plan_project_note_item(
            request,
            parsed,
            now,
            today,
            roll_seed,
            parsed_item.index,
            planner,
        );
    }
    let created = date_string(today);
    let priority = match parsed.priority_level {
        Some(number) => Some(resolve_priority(
            number,
            parsed.scheduled_offset,
            item_roll_seed(roll_seed, parsed_item.index),
            config_path,
        )?),
        None => None,
    };
    let priority_field = priority
        .as_ref()
        .map(|resolved| (resolved.name.as_str(), resolved.value.as_str()));
    let schedule_log_reason = priority.as_ref().and_then(|resolved| {
        resolved.rolled_offset.map(|rolled_days| {
            capture_schedule_log::priority_roll_reason(
                capture_schedule_log::IMPLICIT_LEVEL_LABEL,
                &resolved.label,
                rolled_days,
                resolved.min_days,
                resolved.max_days,
            )
        })
    });
    let scheduled_offset = parsed.scheduled_offset.or_else(|| {
        priority
            .as_ref()
            .and_then(|resolved| resolved.rolled_offset)
    });
    let scheduled = scheduled_offset
        .map(|offset| scheduled_date_string(today, offset))
        .transpose()?;
    // A9: reserve N's ID before the capture line is built, even when
    // `dependencies.blocks_lines` is off. Typed `^id` / `:id` win.
    // `~~` always mints N so the source's LINKS entry can name it.
    let mut minted_blocker_block_id: Option<MintedDependentBlockIdJson> = None;
    let mut minted_relation_block_ids: Vec<MintedRelationBlockIdJson> =
        Vec::new();
    let needs_new_task_id = matches!(parsed.kind, CaptureKind::Task)
        && (!parsed.blocked_tasks.is_empty()
            || parsed
                .relations
                .iter()
                .any(|relation| relation.direction == RelationDirection::From));
    if needs_new_task_id {
        let dest = relative_target(parsed.route.as_deref());
        let absolute = request.bob_dir.join(&dest);
        let minted = mint_staged_block_id(
            dependency_ctx,
            planner,
            &absolute,
            &dest,
            &format!("- [ ] #task {}", parsed.body),
        )?;
        if !parsed.blocked_tasks.is_empty() {
            minted_blocker_block_id = Some(MintedDependentBlockIdJson {
                path: display_relative(&dest),
                line: 0,
                block_id: minted.clone(),
            });
        }
        if parsed
            .relations
            .iter()
            .any(|relation| relation.direction == RelationDirection::From)
        {
            minted_relation_block_ids.push(MintedRelationBlockIdJson {
                path: display_relative(&dest),
                line: 0,
                block_id: minted.clone(),
                role: MintedRelationRole::Target,
            });
        }
        parsed.kind = CaptureKind::TaskWithBlockId {
            block_id: minted.clone(),
        };
        if let Some(target) = parsed.dependency_target.as_mut() {
            target.block_id = Some(minted);
        }
    }
    let capture_line = match &parsed.kind {
        CaptureKind::Task => format_task_line(
            &parsed.body,
            &created,
            priority_field,
            scheduled.as_deref(),
        ),
        CaptureKind::TaskWithBlockId { block_id } => {
            format_task_with_block_id_line(
                &parsed.body,
                &created,
                priority_field,
                scheduled.as_deref(),
                block_id,
            )
        }
        CaptureKind::Bullet { .. } => format_bullet_line(
            &parsed.body,
            &created,
            priority_field,
            scheduled.as_deref(),
        ),
        CaptureKind::SubBullet { .. } => format_sub_bullet_line(
            &parsed.body,
            priority_field,
            scheduled.as_deref(),
        ),
        CaptureKind::Pomodoro { block_id, .. } => format_pomodoro_task_line(
            &parsed.body,
            &created,
            priority_field,
            scheduled.as_deref(),
            block_id,
        ),
        CaptureKind::PomodoroNote => {
            format_sub_bullet_line(&parsed.body, None, None)
        }
        CaptureKind::ProjectNote { .. } => {
            unreachable!(
                "project-note capture is planned by plan_project_note_item"
            )
        }
        CaptureKind::TaskToggle { .. } => {
            unreachable!("task toggle capture is rejected before this point")
        }
        CaptureKind::PomodoroAdjust { .. } => {
            unreachable!(
                "pomodoro adjustment capture is planned before this point"
            )
        }
        CaptureKind::PomodoroShift { .. } => {
            unreachable!("pomodoro shift capture is planned before this point")
        }
        CaptureKind::PomodoroLink { .. } => {
            unreachable!("pomodoro link capture is planned before this point")
        }
        CaptureKind::PomodoroClose { .. } => {
            unreachable!("pomodoro close capture is planned before this point")
        }
        CaptureKind::PomodoroStart { .. } => {
            unreachable!("pomodoro start capture is planned before this point")
        }
        CaptureKind::TaskComplete { .. } => {
            unreachable!("task complete capture is planned before this point")
        }
        CaptureKind::Ref(_) => {
            unreachable!("reference capture is planned before this point")
        }
    };
    let kind_label = capture_kind_label(&parsed.kind);
    let mut task_block_id = match &parsed.kind {
        CaptureKind::TaskWithBlockId { block_id } => Some(block_id.clone()),
        // A `:` task reports its id even when the guard creates it
        // unlinked (no Pomodoro write to read it back from).
        CaptureKind::Pomodoro { block_id, .. } => Some(block_id.clone()),
        _ => None,
    };
    let (relative_target, target) = match &parsed.kind {
        CaptureKind::PomodoroNote => {
            let day_file = pomodoro::day_file_for(&request.bob_dir);
            let relative_target = day_file
                .strip_prefix(&request.bob_dir)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| day_file.clone());
            (relative_target, day_file)
        }
        _ => {
            let relative_target = relative_target(parsed.route.as_deref());
            let target = request.bob_dir.join(&relative_target);
            (relative_target, target)
        }
    };
    let child_indent = (parsed.clip.is_some()
        || schedule_log_reason.is_some()
        || !parsed.sub_bullets.is_empty())
    .then(|| child_indent_unit(planner, &target))
    .transpose()?;
    let sub_bullet_lines: Vec<String> = if parsed.sub_bullets.is_empty() {
        Vec::new()
    } else {
        let indent = child_indent.as_deref().unwrap_or("\t");
        render_authored_sub_bullets(&parsed.sub_bullets, indent)
    };
    let clip_plan = match parsed.clip.as_ref() {
        Some(ClipRequest::Current { header }) => {
            let clipboard =
                capture_clip::read_clipboard().map_err(CaptureError::io)?;
            Some(
                capture_clip::plan_with_reservations(
                    &request.bob_dir,
                    header.as_deref(),
                    &clipboard,
                    now,
                    child_indent.as_deref().unwrap_or("\t"),
                    clip_reservations,
                )
                .map_err(CaptureError::io)?,
            )
        }
        Some(ClipRequest::History { count }) if count.get() == 1 => {
            let clipboard =
                capture_clip::read_clipboard().map_err(CaptureError::io)?;
            Some(
                capture_clip::plan_with_reservations(
                    &request.bob_dir,
                    None,
                    &clipboard,
                    now,
                    child_indent.as_deref().unwrap_or("\t"),
                    clip_reservations,
                )
                .map_err(CaptureError::io)?,
            )
        }
        Some(ClipRequest::History { count }) => {
            let clipboards = capture_clip::read_clipboard_history(count.get())
                .map_err(CaptureError::io)?;
            Some(
                capture_clip::plan_history_with_reservations(
                    &request.bob_dir,
                    &clipboards,
                    now,
                    child_indent.as_deref().unwrap_or("\t"),
                    clip_reservations,
                )
                .map_err(CaptureError::io)?,
            )
        }
        None => None,
    };
    let clip_output = clip_plan.as_ref().map(|plan| plan.output.clone());
    let schedule_log = schedule_log_reason.and_then(|reason| {
        scheduled.as_deref().map(|scheduled| {
            capture_schedule_log::plan(
                child_indent.as_deref().unwrap_or("\t"),
                scheduled,
                reason,
            )
        })
    });
    // Dependencies for a task this capture creates augment the block
    // before insertion: the managed child leads the authored children,
    // so duplicate bodies in one batch each land correctly.
    let new_task_dependencies = match &parsed.dependency_target {
        Some(target)
            if target.kind == ParsedDependencyTargetKind::NewTask
                && matches!(
                    parsed.kind,
                    CaptureKind::Task
                        | CaptureKind::TaskWithBlockId { .. }
                        | CaptureKind::Pomodoro { .. }
                ) =>
        {
            if parsed.dependencies.is_empty() {
                None
            } else {
                let status = match &parsed.kind {
                    CaptureKind::Pomodoro { .. } => '*',
                    _ => scheduled.as_deref().map(|_| '?').unwrap_or(' '),
                };
                let (augmented, parts) = plan_new_task_dependencies(
                    dependency_ctx,
                    planner,
                    &request.bob_dir,
                    parsed.route.as_deref(),
                    target.block_id.as_deref(),
                    &capture_line,
                    status,
                    child_indent.as_deref(),
                    &parsed.dependencies,
                    warnings,
                )?;
                Some((augmented, parts))
            }
        }
        Some(target) if target.kind == ParsedDependencyTargetKind::NewTask => {
            return Err(CaptureError::io(
                "dependency invariant failed: new-task target on a non-task capture",
            ));
        }
        _ => None,
    };
    // Link guard for new tasks (§13.5, PDG4–PDG5): a task captured
    // into a blocked project lands `[?]` plus `#prj_is_blocked`, and
    // a `:` task that is Blocked after the gesture (own future
    // schedule, open `&` prerequisites, or project reasons) is
    // created unlinked with its seeds promoted.
    let mut new_task_dependencies = new_task_dependencies;
    let mut new_task_project_block: Option<ConsumePlan> = None;
    if matches!(
        parsed.kind,
        CaptureKind::Task
            | CaptureKind::TaskWithBlockId { .. }
            | CaptureKind::Pomodoro { .. }
    ) {
        // The `relative_target` binding below shadows the helper
        // of the same name; this inlines `relative_target(route)`.
        let dest_rel = parsed
            .route
            .as_deref()
            .map(|route| PathBuf::from(route_label(route)))
            .unwrap_or_else(|| PathBuf::from(INBOX_FILE));
        new_task_project_block = assess_destination_project(
            dependency_ctx,
            planner,
            &request.bob_dir,
            &dest_rel,
            today,
        )?;
        if new_task_project_block.is_some() {
            let status_types =
                note_tasks::read_settings(&request.bob_dir).status_types;
            let flip = |line: &str| {
                apply_new_task_project_block(line, &status_types)
                    .unwrap_or_else(|| line.to_string())
            };
            if let Some((augmented, _)) = &mut new_task_dependencies {
                augmented.task_line = flip(&augmented.task_line);
            }
            // Without `&` dependencies the plain capture line flips
            // below via `flipped_capture_line`.
        }
    }
    // Plain capture lines (no augmented dependency block) flip here;
    // augmented lines flipped above.
    let flipped_capture_line =
        match (&new_task_dependencies, &new_task_project_block) {
            (None, Some(_)) => {
                let status_types =
                    note_tasks::read_settings(&request.bob_dir).status_types;
                apply_new_task_project_block(&capture_line, &status_types)
                    .unwrap_or_else(|| capture_line.clone())
            }
            _ => capture_line.clone(),
        };
    let (mut block_first_line, block_child_lines) = match &new_task_dependencies
    {
        Some((augmented, _)) => {
            let mut children = vec![augmented.dep_child.clone()];
            children.extend(sub_bullet_lines.iter().cloned());
            (augmented.task_line.clone(), children)
        }
        None => (flipped_capture_line.clone(), sub_bullet_lines.clone()),
    };
    let capture_block = assemble_capture_block(
        &block_first_line,
        (!block_child_lines.is_empty()).then_some(block_child_lines.as_slice()),
        clip_plan.as_ref().map(|plan| plan.output.lines.as_slice()),
        schedule_log.as_ref().map(|log| log.lines.as_slice()),
    );
    // Set when a `:` task is created unlinked below (PDG4/PDG5).
    let mut new_task_link_guard: Option<LinkGuardJson> = None;
    let mut note_plan = match &parsed.kind {
        CaptureKind::SubBullet {
            target: sub_bullet_target,
            section,
        } => {
            let route = parsed.route.as_deref().ok_or_else(|| {
                CaptureError::io(
                    "sub-bullet capture invariant failed: route is missing",
                )
            })?;
            plan_sub_bullet_capture(
                planner,
                &request.bob_dir,
                &target,
                route,
                sub_bullet_target,
                section.as_ref(),
                &capture_block,
            )?
        }
        CaptureKind::Pomodoro {
            block_id,
            pomodoro_name,
            pomodoro_name_is_literal,
            start,
            close,
        } => {
            let route = parsed.route.as_deref().ok_or_else(|| {
                CaptureError::io(
                    "Pomodoro capture invariant failed: route is missing",
                )
            })?;
            if let Some(close_spec) = close.clone() {
                if !parsed.blocked_tasks.is_empty() {
                    return Err(CaptureError::usage(
                        capture_language::close_blocked_task_error(
                            &close_star_marker(&parsed),
                        ),
                    ));
                }
                // The augmented block already carries the managed child;
                // the close path only needs the summary parts.
                let close_dependencies =
                    new_task_dependencies.as_ref().map(|(augmented, parts)| {
                        (augmented.task_line.clone(), parts.clone())
                    });
                return plan_pomodoro_close_task_item(
                    request,
                    parsed.clone(),
                    now,
                    today,
                    planner,
                    warnings,
                    route,
                    block_id,
                    &close_spec,
                    &capture_block,
                    close_dependencies,
                    dependency_ctx,
                );
            }
            // PDG4/PDG5: a `:` task that is Blocked after the
            // gesture is created unlinked; its seeds rise instead.
            // (The close arm above is exempt: links under the
            // Pomodoro the same gesture closes are history.)
            let mut blocked_reasons: Vec<BlockedReason> = Vec::new();
            if let Some(date) = scheduled.as_deref()
                && date > date_string(today).as_str()
            {
                blocked_reasons.push(BlockedReason {
                    kind: "local_schedule",
                    label: format!("scheduled {date}"),
                });
            }
            if let Some((_, parts)) = new_task_dependencies.as_ref() {
                let open: Vec<String> = parts
                    .summary
                    .prerequisites
                    .iter()
                    .filter(|prereq| prereq.open)
                    .map(|prereq| {
                        format!("[[{}#^{}]]", prereq.note, prereq.block_id)
                    })
                    .collect();
                if !open.is_empty() {
                    blocked_reasons.push(BlockedReason {
                        kind: "local_dependency",
                        label: format!("waits on {}", open.join(", ")),
                    });
                }
            }
            if let Some(project) = &new_task_project_block {
                blocked_reasons.extend(project.reasons.iter().cloned());
            }
            if blocked_reasons.is_empty() {
                plan_capture_with_pomodoro_link(
                    planner,
                    &request.bob_dir,
                    &target,
                    route,
                    block_id,
                    pomodoro_name.as_deref(),
                    *pomodoro_name_is_literal,
                    start.as_ref(),
                    &capture_block,
                    now,
                )?
            } else {
                let mut promoted = Vec::new();
                if let Some(project) = new_task_project_block {
                    // `&` seeds already rose inside
                    // `plan_new_task_dependencies`; only the project's
                    // seeds still need promotion.
                    promoted = promote_consume_plan(
                        dependency_ctx,
                        planner,
                        &request.bob_dir,
                        project,
                        &format!("new task ^{block_id}"),
                    )?;
                }
                new_task_link_guard = Some(LinkGuardJson::not_linked(
                    &blocked_reasons,
                    promoted,
                    None,
                ));
                plan_capture_to_target(
                    planner,
                    &target,
                    &capture_block,
                    &parsed.kind,
                )?
            }
        }
        CaptureKind::PomodoroNote => {
            plan_pomodoro_note_capture(planner, &target, &capture_block)?
        }
        CaptureKind::ProjectNote { .. } => {
            return Err(CaptureError::io(
                "project-note capture invariant failed: wrong write planner",
            ));
        }
        _ => plan_capture_to_target(
            planner,
            &target,
            &capture_block,
            &parsed.kind,
        )?,
    };
    let mut pomodoro_refs = std::mem::take(&mut note_plan.pomodoro_refs);
    // Dependency effects report through `task_blocks` with the new
    // `dependency` / `dependency_target` roles, and through the
    // `dependency_update` / `dependency_updates[]` preview. BLOCKS-mirror
    // rows ride additively as `blocks_line_updates` plus the singular
    // `minted_dependent_block_id` (§14.11). `*` mints report separately
    // as `minted_blocker_block_id`.
    let mut dependency_summaries: Vec<DependencyUpdateJson> = Vec::new();
    let mut blocks_line_updates: Vec<BlocksLineUpdateJson> = Vec::new();
    let mut minted_dependent_block_id: Option<MintedDependentBlockIdJson> =
        None;
    let mut dependency_block_refs: Vec<TaskBlockRef> = Vec::new();
    match &parsed.dependency_target {
        Some(target) if target.kind == ParsedDependencyTargetKind::NewTask => {
            if let Some((augmented, parts)) = &new_task_dependencies {
                if let Some(tracked) = locate_new_task_ref(
                    planner,
                    &request.bob_dir,
                    parts,
                    &augmented.task_line,
                ) {
                    if let Some(minted) = parts.blocks.minted.clone() {
                        minted_dependent_block_id =
                            Some(MintedDependentBlockIdJson {
                                line: tracked.line + 1,
                                ..minted
                            });
                    }
                    dependency_block_refs.push(tracked);
                } else if let Some(minted) = parts.blocks.minted.clone() {
                    minted_dependent_block_id = Some(minted);
                }
                dependency_block_refs.extend(parts.target_refs.clone());
                pomodoro_refs.extend(parts.pomodoro_refs.iter().copied());
                dependency_summaries.push(parts.summary.clone());
                blocks_line_updates = parts.blocks.updates.clone();
            }
        }
        Some(target)
            if target.kind == ParsedDependencyTargetKind::ExistingTask
                && matches!(parsed.kind, CaptureKind::SubBullet { .. }) =>
        {
            let (route, block_id) = match target {
                ParsedDependencyTarget {
                    route: Some(route),
                    block_id: Some(block_id),
                    ..
                } => (route.clone(), block_id.clone()),
                _ => {
                    return Err(CaptureError::io(
                        "dependency invariant failed: existing-task target without route and block ID",
                    ));
                }
            };
            // Prose captures normally first; the modifiers belong to the
            // explicit parent task, outside any child section. `&` first,
            // then `*` with no Next cap.
            if !parsed.dependencies.is_empty() {
                let update = plan_existing_task_dependencies(
                    dependency_ctx,
                    planner,
                    &request.bob_dir,
                    &DependentRef::from_route(&route, &block_id),
                    &parsed.dependencies,
                    warnings,
                    ExistingDependencyOptions::ampersand(),
                )?;
                absorb_dependency_write(
                    update,
                    &mut dependency_summaries,
                    &mut dependency_block_refs,
                    &mut pomodoro_refs,
                    &mut blocks_line_updates,
                );
            }
        }
        _ => {}
    }
    if !parsed.blocked_tasks.is_empty() {
        let owner = star_owner_ref(
            &parsed,
            &relative_target,
            task_block_id.as_deref(),
            minted_dependent_block_id.as_ref(),
            minted_blocker_block_id.as_ref(),
        )?;
        let options = match &parsed.dependency_target {
            Some(target)
                if target.kind == ParsedDependencyTargetKind::ExistingTask =>
            {
                ExistingDependencyOptions::star_existing_owner()
            }
            _ => ExistingDependencyOptions::star_new_blocker(),
        };
        let writes = plan_blocked_task_writes(
            dependency_ctx,
            planner,
            &request.bob_dir,
            &owner,
            &parsed.blocked_tasks,
            warnings,
            options,
        )?;
        for write in writes {
            absorb_dependency_write(
                write,
                &mut dependency_summaries,
                &mut dependency_block_refs,
                &mut pomodoro_refs,
                &mut blocks_line_updates,
            );
        }
        let relocated = locate_task_by_block_id(
            dependency_ctx,
            planner,
            &request.bob_dir,
            &owner.relative,
            &owner.block_id,
        )?;
        if let Some(minted) = minted_blocker_block_id.as_mut() {
            minted.line = relocated.line + 1;
        }
        if matches!(
            parsed.kind,
            CaptureKind::Task
                | CaptureKind::TaskWithBlockId { .. }
                | CaptureKind::Pomodoro { .. }
        ) {
            let contents = current_note_text(planner, &relocated.target)?;
            if let Some(line) = contents.lines().nth(relocated.line) {
                block_first_line = line.to_string();
            }
            if task_block_id.is_none() {
                task_block_id = relocated.block_id.clone();
            }
        }
    }
    let mut relation_updates: Vec<RelationUpdateJson> = Vec::new();
    let mut relation_cancels: Vec<RelationCancelJson> = Vec::new();
    let mut relation_completes: Vec<RelationCompleteJson> = Vec::new();
    let mut close_day_file: Option<String> = None;
    let mut close_task_block_refs: Vec<TaskBlockRef> = Vec::new();
    let mut linked_from_line_updates: Vec<BlocksLineUpdateJson> = Vec::new();
    let mut mirror_skipped: Vec<MirrorSkippedJson> = Vec::new();
    if !parsed.relations.is_empty() {
        let owner = locate_relation_owner(
            dependency_ctx,
            planner,
            &request.bob_dir,
            &parsed,
            &relative_target,
            task_block_id.as_deref(),
            minted_dependent_block_id.as_ref(),
            minted_blocker_block_id.as_ref(),
            &block_first_line,
        )?;
        let existing_owner = matches!(
            parsed.dependency_target.as_ref().map(|target| target.kind),
            Some(ParsedDependencyTargetKind::ExistingTask)
        );
        let reports = plan_relation_writes(
            dependency_ctx,
            planner,
            &request.bob_dir,
            &owner,
            &parsed.relations,
            warnings,
            existing_owner,
        )?;
        relation_updates = reports.updates;
        linked_from_line_updates = reports.linked_from;
        minted_relation_block_ids.extend(reports.minted);
        mirror_skipped = reports.skipped;
        dependency_block_refs.extend(reports.source_refs);
        let close = plan_relation_close_effects(
            dependency_ctx,
            planner,
            request,
            &parsed,
            &mut relation_updates,
            existing_owner,
            warnings,
        )?;
        relation_cancels = close.cancels;
        relation_completes = close.completes;
        pomodoro_refs.extend(close.pomodoro_refs);
        close_task_block_refs = close.task_block_refs;
        close_day_file = close.day_file;
        if let Some(block_id) = reports.owner_block_id.as_deref() {
            let relocated = locate_task_by_block_id(
                dependency_ctx,
                planner,
                &request.bob_dir,
                &owner.relative,
                block_id,
            )?;
            for minted in &mut minted_relation_block_ids {
                if minted.block_id == block_id {
                    minted.line = relocated.line + 1;
                    minted.path = relocated.relative_target.clone();
                }
            }
            if matches!(
                parsed.kind,
                CaptureKind::Task
                    | CaptureKind::TaskWithBlockId { .. }
                    | CaptureKind::Pomodoro { .. }
            ) {
                let contents = current_note_text(planner, &relocated.target)?;
                if let Some(line) = contents.lines().nth(relocated.line) {
                    block_first_line = line.to_string();
                }
                if task_block_id.is_none() {
                    task_block_id = relocated.block_id.clone();
                }
            }
        }
    }
    // Sub-bullet `under` uses the parent's final status, including `&`
    // writes that never enter the `*` block above.
    if let Some(details) = note_plan.sub_bullet.as_mut()
        && !dependency_summaries.is_empty()
        && let Some(target) = parsed.dependency_target.as_ref()
        && target.kind == ParsedDependencyTargetKind::ExistingTask
        && let (Some(route), Some(block_id)) =
            (target.route.as_deref(), target.block_id.as_deref())
    {
        let owner = DependentRef::from_route(route, block_id);
        let relocated = locate_task_by_block_id(
            dependency_ctx,
            planner,
            &request.bob_dir,
            &owner.relative,
            &owner.block_id,
        )?;
        let contents = current_note_text(planner, &relocated.target)?;
        let scan = note_tasks::scan(&contents, dependency_ctx.settings());
        if let Ok(task) = lookup_staged_task(
            &scan,
            &relocated.relative_target,
            &owner.block_id,
        ) {
            details.parent_status_symbol = task.status_symbol;
            details.parent_status_name = task.status_name.clone();
            details.parent_line = task.line_index + 1;
        }
    }
    let task_theme = match &parsed.kind {
        CaptureKind::Pomodoro {
            block_id,
            pomodoro_name: Some(_),
            close: None,
            ..
        } if new_task_link_guard.is_none() && note_plan.pomodoro.is_some() => {
            let route = parsed.route.as_deref().ok_or_else(|| {
                CaptureError::io(
                    "Pomodoro capture invariant failed: route is missing",
                )
            })?;
            let destination_name = note_plan
                .pomodoro
                .as_ref()
                .and_then(|edit| edit.details.pomodoro_name.as_deref());
            task_theme_context
                .apply_new_task(
                    planner,
                    &target,
                    route,
                    block_id,
                    destination_name,
                    warnings,
                )?
                .map(|applied| {
                    block_first_line = applied.task_line;
                    applied.summary
                })
        }
        _ => None,
    };
    let (dependency_update, dependency_updates) =
        fold_dependency_updates(dependency_summaries);
    dependency_block_refs = relocate_task_block_refs(
        dependency_ctx,
        planner,
        &request.bob_dir,
        dependency_block_refs,
    )?;
    // One-off `||` labels hide after the notes commit succeeds. The
    // hides ride the plan so dry runs preview them; the commit appends
    // one `relation_label.hide` per distinct key that is not already
    // hidden. Counts come from the pre-capture catalog.
    let (relation_label_hides, relation_hide_warnings) =
        plan_relation_label_hides(&request.bob_dir, &parsed.relations);
    warnings.extend(relation_hide_warnings);
    let special = note_plan.pomodoro.as_ref();
    let sub_bullet = note_plan.sub_bullet.as_ref();
    let pomodoro_note = note_plan.pomodoro_note.as_ref();
    let task_block_refs = match &parsed.kind {
        CaptureKind::SubBullet { .. } => {
            if let Some(details) = note_plan.sub_bullet.as_ref() {
                let route_str =
                    parsed.route.as_deref().unwrap_or("").to_string();
                let parent_index = details.parent_line.saturating_sub(1);
                if cfg!(debug_assertions)
                    && let Some(post) = planner.peek_text(&target)
                {
                    let settings = note_tasks::read_settings(&request.bob_dir);
                    let scan = note_tasks::scan(&post, &settings);
                    debug_assert!(
                        scan.task_at(parent_index).is_some(),
                        "sub-bullet parent missing in post-state"
                    );
                }
                vec![TaskBlockRef {
                    target: target.clone(),
                    relative_target: relative_target
                        .to_string_lossy()
                        .into_owned(),
                    route: route_str,
                    line: parent_index,
                    block_id: details.block_id.clone(),
                    role: TaskBlockRole::SubBullet,
                }]
            } else {
                debug_assert!(false, "sub-bullet missing details");
                Vec::new()
            }
        }
        _ => Vec::new(),
    };

    Ok(PlannedCaptureItem {
        result: CaptureItemResult {
            ok: true,
            dry_run: request.dry_run,
            routed: parsed.route.is_some(),
            route_label: parsed
                .route
                .as_deref()
                .map(route_label)
                .unwrap_or_default(),
            route: parsed.route,
            relative_target: relative_target.to_string_lossy().into_owned(),
            target: target.display().to_string(),
            text: parsed.body,
            task_line: block_first_line,
            task_theme,
            kind: kind_label,
            created,
            scheduled,
            priority: priority.as_ref().map(|resolved| resolved.value.clone()),
            priority_label: priority
                .as_ref()
                .map(|resolved| resolved.label.clone()),
            placement: note_plan.placement,
            sub_bullets: sub_bullet_lines,
            clip: clip_output,
            schedule_log,
            block_id: task_block_id
                .or_else(|| {
                    special.as_ref().map(|edit| edit.details.block_id.clone())
                })
                .or_else(|| sub_bullet.and_then(|edit| edit.block_id.clone())),
            day_file: special
                .as_ref()
                .map(|edit| edit.details.day_file.clone())
                .or_else(|| pomodoro_note.map(|note| note.day_file.clone()))
                .or(close_day_file),
            block_link: special
                .as_ref()
                .map(|edit| edit.details.block_link.clone()),
            pomodoro_link_placement: special
                .as_ref()
                .map(|edit| edit.details.pomodoro_link_placement),
            parent_line: sub_bullet
                .map(|edit| edit.parent_line)
                .or_else(|| pomodoro_note.map(|note| note.pomodoro_line)),
            parent_text: sub_bullet
                .map(|edit| edit.parent_text.clone())
                .or_else(|| {
                    pomodoro_note.map(|note| note.pomodoro_text.clone())
                }),
            parent_section: sub_bullet
                .and_then(|edit| edit.parent_section.clone()),
            parent_status_symbol: sub_bullet
                .map(|edit| edit.parent_status_symbol),
            parent_status_name: sub_bullet
                .map(|edit| edit.parent_status_name.clone()),
            toggle_direction: None,
            previous_task_line: None,
            status_symbol: None,
            status_name: None,
            previous_status_symbol: None,
            previous_status_name: None,
            pomodoro_name: special
                .as_ref()
                .and_then(|edit| edit.details.pomodoro_name.clone()),
            creates_pomodoro: special
                .as_ref()
                .map(|edit| edit.details.creates_pomodoro),
            pomodoro_already_linked: None,
            removed_pomodoro_links: None,
            removed_scheduled: None,
            pomodoro_selector_unused: None,
            toggle_behavior: None,
            status_changed: None,
            pomodoro_link_action: None,
            pomodoro_link_source: None,
            pomodoro_link_destination: special.as_ref().and_then(|edit| {
                edit.details.pomodoro_link_destination.clone()
            }),
            project_note: None,
            pomodoro_start: special
                .as_ref()
                .and_then(|edit| edit.start.clone()),
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_close: None,
            pomodoro_reset: None,
            dependency_update,
            consumed_links: Vec::new(),
            task_complete: None,
            toggle_task_description: None,
            r#ref: None,
            link_guard: new_task_link_guard,
            dropped_dependencies: Vec::new(),
            blocks_line_updates,
            minted_dependent_block_id,
            dependency_updates,
            minted_blocker_block_id,
            relation_updates,
            relation_cancels,
            relation_completes,
            linked_from_line_updates,
            minted_relation_block_ids,
            mirror_skipped,
            relation_label_hides,
        },
        clip_plan,
        pomodoro_refs,
        task_block_refs: {
            let mut refs = task_block_refs;
            refs.extend(dependency_block_refs);
            refs.extend(close_task_block_refs);
            refs
        },
    })
}

pub(super) fn item_roll_seed(base: u64, item_index: usize) -> u64 {
    base.wrapping_add((item_index as u64).wrapping_mul(0x9E3779B97F4A7C15))
}

/// Join the capture line with its authored children (if any), its clip
/// children (if any), and its schedule log lines (if any), in note order:
/// the captured line first, then the authored bullets the user typed
/// beneath it, then any clipboard children, and the schedule log last since
/// it documents the whole block above it.
pub(super) fn assemble_capture_block(
    capture_line: &str,
    sub_bullet_lines: Option<&[String]>,
    clip_lines: Option<&[String]>,
    schedule_log_lines: Option<&[String]>,
) -> String {
    let mut lines = vec![capture_line];
    if let Some(sub_bullet_lines) = sub_bullet_lines {
        lines.extend(sub_bullet_lines.iter().map(String::as_str));
    }
    if let Some(clip_lines) = clip_lines {
        lines.extend(clip_lines.iter().map(String::as_str));
    }
    if let Some(schedule_log_lines) = schedule_log_lines {
        lines.extend(schedule_log_lines.iter().map(String::as_str));
    }
    lines.join("\n")
}

pub(super) fn render_authored_sub_bullets(
    sub_bullets: &[AuthoredSubBullet],
    indent_unit: &str,
) -> Vec<String> {
    sub_bullets
        .iter()
        .map(|item| {
            let indentation = indent_unit.repeat(item.depth.indent_units());
            format!("{indentation}- {}", item.body)
        })
        .collect()
}

pub(super) fn capture_kind_label(kind: &CaptureKind) -> &'static str {
    match kind {
        CaptureKind::Task | CaptureKind::TaskWithBlockId { .. } => "task",
        CaptureKind::Bullet { .. } => "bullet",
        CaptureKind::Pomodoro { .. } => "pomodoro_task",
        CaptureKind::SubBullet { .. } => "sub_bullet",
        CaptureKind::PomodoroNote => "pomodoro_note",
        CaptureKind::ProjectNote { .. } => "project_note",
        CaptureKind::TaskToggle { .. } => "task_toggle",
        CaptureKind::PomodoroAdjust { .. } => "pomodoro_adjust",
        CaptureKind::PomodoroShift { .. } => "pomodoro_shift",
        CaptureKind::PomodoroLink { .. } => "pomodoro_link",
        CaptureKind::PomodoroClose { .. } => "pomodoro_close",
        CaptureKind::PomodoroStart { .. } => "pomodoro_start",
        CaptureKind::TaskComplete { .. } => "task_complete",
        CaptureKind::Ref(_) => "ref",
    }
}

pub(super) fn date_string(date: NaiveDate) -> String {
    format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
}

pub(super) fn scheduled_date_string(
    today: NaiveDate,
    offset_days: u64,
) -> Result<String, CaptureError> {
    let scheduled =
        today
            .checked_add_days(Days::new(offset_days))
            .ok_or_else(|| {
                CaptureError::usage("scheduled offset is out of range")
            })?;
    Ok(date_string(scheduled))
}

/// A resolved `$<N>` level: its field name/value/label plus the level's
/// roll window and, when the date was actually rolled, the chosen offset.
pub(super) struct ResolvedPriority {
    pub(super) name: String,
    pub(super) value: String,
    pub(super) label: String,
    pub(super) min_days: u64,
    pub(super) max_days: u64,
    pub(super) rolled_offset: Option<u64>,
}

/// Resolve a `$<N>` level. The rolled offset is only computed when no
/// explicit `s:<N>` offset is present, since an explicit offset always wins
/// the scheduled date.
pub(super) fn resolve_priority(
    number: u64,
    explicit_scheduled_offset: Option<u64>,
    roll_seed: u64,
    config_path: Option<&Path>,
) -> Result<ResolvedPriority, CaptureError> {
    let path = config_path
        .map(Path::to_path_buf)
        .unwrap_or_else(config::config_path);
    let property =
        config::load_priority_property(&path).map_err(|error| match error {
            config::ConfigError::Read(message) => CaptureError::io(message),
            config::ConfigError::Invalid(message) => {
                CaptureError::usage(message)
            }
        })?;
    let level = property.level(number).ok_or_else(|| {
        CaptureError::usage(format!(
            "${number} is not a configured priority level; use $ or $1 through ${} ({})",
            property.level_count(),
            property.labels()
        ))
    })?;
    let rolled_offset = explicit_scheduled_offset
        .is_none()
        .then(|| level.roll_offset(roll_seed));
    Ok(ResolvedPriority {
        name: property.name().to_string(),
        value: level.value().to_string(),
        label: level.label().to_string(),
        min_days: level.min_days(),
        max_days: level.max_days(),
        rolled_offset,
    })
}

fn typed_relationship_phrase(parsed: &ParsedCaptureText) -> String {
    let mut parts = Vec::new();
    if !parsed.dependencies.is_empty() {
        parts.push("'&'");
    }
    if !parsed.blocked_tasks.is_empty() {
        parts.push("'*'");
    }
    if parsed
        .relations
        .iter()
        .any(|relation| relation.direction == RelationDirection::To)
    {
        parts.push("'~'");
    }
    if parsed
        .relations
        .iter()
        .any(|relation| relation.direction == RelationDirection::From)
    {
        parts.push("'~~'");
    }
    match parts.as_slice() {
        [] => "task dependencies".to_string(),
        [one] => (*one).to_string(),
        [a, b] => format!("{a} and {b}"),
        _ => {
            let (last, rest) = parts.split_last().expect("len>=3");
            format!("{} and {last}", rest.join(", "))
        }
    }
}

fn close_star_marker(parsed: &ParsedCaptureText) -> String {
    match &parsed.kind {
        CaptureKind::Pomodoro { block_id, .. } => {
            let route = parsed.route.as_deref().unwrap_or("route");
            format!("@{route}:{block_id}=x")
        }
        _ => parsed
            .route
            .as_deref()
            .map(|route| format!("@{route}:id=x"))
            .unwrap_or_else(|| "@route:id=x".to_string()),
    }
}

/// Existing-task relationship with no body, authored children, or
/// extra capture kind: write the edge only.
fn relationship_only_existing_owner(
    parsed: &ParsedCaptureText,
) -> Option<ParsedDependencyTarget> {
    if !parsed.body.trim().is_empty() || !parsed.sub_bullets.is_empty() {
        return None;
    }
    let target = parsed.dependency_target.as_ref()?;
    if target.kind != ParsedDependencyTargetKind::ExistingTask {
        return None;
    }
    match &parsed.kind {
        CaptureKind::TaskToggle { .. }
        | CaptureKind::PomodoroLink { .. }
        | CaptureKind::SubBullet {
            target: SubBulletTarget::BlockId(_),
            ..
        } => Some(target.clone()),
        _ => None,
    }
}

/// N for a `*` pass: an existing `@route+id` owner, else the new task
/// this item just inserted (typed or minted `^id`).
fn star_owner_ref(
    parsed: &ParsedCaptureText,
    relative_target: &Path,
    task_block_id: Option<&str>,
    minted_dependent: Option<&MintedDependentBlockIdJson>,
    minted_blocker: Option<&MintedDependentBlockIdJson>,
) -> Result<DependentRef, CaptureError> {
    if let Some(target) = parsed.dependency_target.as_ref()
        && target.kind == ParsedDependencyTargetKind::ExistingTask
    {
        return match (target.route.as_deref(), target.block_id.as_deref()) {
            (Some(route), Some(block_id)) => {
                Ok(DependentRef::from_route(route, block_id))
            }
            _ => Err(CaptureError::io(
                "star capture invariant failed: existing owner without route and block ID",
            )),
        };
    }
    let block_id = task_block_id
        .map(str::to_string)
        .or_else(|| minted_dependent.map(|minted| minted.block_id.clone()))
        .or_else(|| minted_blocker.map(|minted| minted.block_id.clone()))
        .ok_or_else(|| {
            CaptureError::io(
                "star capture invariant failed: blocker has no block ID",
            )
        })?;
    Ok(DependentRef {
        relative: relative_target.to_path_buf(),
        block_id,
    })
}

pub(super) fn relative_target(route: Option<&str>) -> PathBuf {
    route
        .map(|route| PathBuf::from(route_label(route)))
        .unwrap_or_else(|| PathBuf::from(INBOX_FILE))
}

pub(crate) fn inbox_route() -> &'static str {
    INBOX_FILE.strip_suffix(".md").unwrap_or(INBOX_FILE)
}

pub(crate) fn route_label(route: &str) -> String {
    format!("{route}.md")
}

pub(crate) fn format_task_line(
    body: &str,
    created: &str,
    priority: Option<(&str, &str)>,
    scheduled: Option<&str>,
) -> String {
    let status = capture_task_status(" ", scheduled);
    let mut line = format!("- [{status}] #task {body} [created::{created}]");
    append_priority_property(&mut line, priority);
    append_scheduled_property(&mut line, scheduled);
    line
}

pub(super) fn format_task_with_block_id_line(
    body: &str,
    created: &str,
    priority: Option<(&str, &str)>,
    scheduled: Option<&str>,
    block_id: &str,
) -> String {
    let mut line = format_task_line(body, created, priority, scheduled);
    append_block_id(&mut line, block_id);
    line
}

pub(super) fn format_bullet_line(
    body: &str,
    created: &str,
    priority: Option<(&str, &str)>,
    scheduled: Option<&str>,
) -> String {
    let mut line = format!("- {body} [created::{created}]");
    append_priority_property(&mut line, priority);
    append_scheduled_property(&mut line, scheduled);
    line
}

pub(super) fn format_sub_bullet_line(
    body: &str,
    priority: Option<(&str, &str)>,
    scheduled: Option<&str>,
) -> String {
    let mut line = format!("- {body}");
    append_priority_property(&mut line, priority);
    append_scheduled_property(&mut line, scheduled);
    line
}

pub(super) fn format_pomodoro_task_line(
    body: &str,
    created: &str,
    priority: Option<(&str, &str)>,
    scheduled: Option<&str>,
    block_id: &str,
) -> String {
    let status = capture_task_status("*", scheduled);
    let mut line = format!("- [{status}] #task {body} [created::{created}]");
    append_priority_property(&mut line, priority);
    append_scheduled_property(&mut line, scheduled);
    append_block_id(&mut line, block_id);
    line
}

pub(super) fn capture_task_status<'a>(
    default: &'a str,
    scheduled: Option<&str>,
) -> &'a str {
    if scheduled.is_some() {
        "?"
    } else {
        default
    }
}

pub(super) fn append_priority_property(
    line: &mut String,
    priority: Option<(&str, &str)>,
) {
    if let Some((property, value)) = priority {
        line.push_str(&format!(" [{property}::{value}]"));
    }
}

pub(super) fn append_scheduled_property(
    line: &mut String,
    scheduled: Option<&str>,
) {
    if let Some(scheduled) = scheduled {
        line.push_str(&format!(" [scheduled::{scheduled}]"));
    }
}

pub(super) fn append_block_id(line: &mut String, block_id: &str) {
    line.push_str(&format!(" ^{block_id}"));
}

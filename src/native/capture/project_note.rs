//! Project-note planning for plus-sigil captures.
use super::batch_move::{apply_batch_move, BatchMoveError};
use super::output::{
    ProjectNoteMoveHint, ProjectNoteMoveSummary, ProjectNoteMovedTask,
    ProjectNotePlannedLink, ProjectNoteRepairedFile,
};
use super::*;
use crate::native::capture_active_tasks;
use crate::native::task_fields;
use crate::native::task_move::{
    movable_roots, resolve_movable, IdCollisionPolicy, Ineligible, MovableRoot,
    MovePlacement, MoveRequest, MoveSelector, PlanError,
};

pub(super) fn reject_project_note_conflicts(
    parsed: &ParsedCaptureText,
    request: &CaptureRequest,
) -> Result<(), CaptureError> {
    if request.forced_clip.is_some() {
        return Err(CaptureError::usage(
            "project-note capture cannot be combined with --clip",
        ));
    }
    if parsed.clip.is_some() {
        return Err(CaptureError::usage(
            "project-note capture cannot be combined with %c",
        ));
    }
    Ok(())
}

pub(super) fn validate_project_note_parent(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    route: &str,
) -> Result<(), CaptureError> {
    let parent_path = bob_dir.join(format!("{route}.md"));
    let Some(contents) = planner.current_contents(&parent_path)? else {
        return Err(CaptureError::io(format!(
            "cannot create a project note under {route}.md: note does not exist (run 'bob capture-targets' to list routable notes)"
        )));
    };
    let Some(frontmatter) = parse_frontmatter(&contents) else {
        return Err(CaptureError::io(format!(
            "cannot create a project note under {route}.md: note is not an area or project note"
        )));
    };
    if frontmatter_is_area(&frontmatter) {
        return Ok(());
    }
    if !frontmatter_is_project(&frontmatter) {
        return Err(CaptureError::io(format!(
            "cannot create a project note under {route}.md: note is not an area or project note"
        )));
    }
    let status =
        ProjectStatus::parse(frontmatter_value(&frontmatter, "status"));
    if status.is_terminal() {
        return Err(CaptureError::io(format!(
            "cannot create a project note under {route}.md: note is a {} project",
            status.label()
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn plan_project_note_item(
    request: &CaptureRequest,
    parsed: ParsedCaptureText,
    now: chrono::NaiveDateTime,
    today: NaiveDate,
    roll_seed: u64,
    item_index: usize,
    planner: &mut CaptureBatchPlanner,
    task_theme_context: &mut CaptureTaskThemeContext,
    warnings: &mut Vec<CaptureWarning>,
    dependency_ctx: &mut DependencyContext,
) -> Result<PlannedCaptureItem, CaptureError> {
    let CaptureKind::ProjectNote {
        block_id,
        pomodoro_name,
        pomodoro_name_is_literal,
        form,
    } = parsed.kind.clone()
    else {
        return Err(CaptureError::io(
            "project-note capture invariant failed: wrong capture kind",
        ));
    };
    let mut parsed = parsed;
    // Interim `cli-grammar` refusal: any `- ^^` is write-free refused until
    // `cli-report` builds the report. `cli-report` deletes this check and
    // its test.
    if parsed
        .sub_bullets
        .iter()
        .any(|sub_bullet| sub_bullet.move_choice.is_some())
    {
        return Err(CaptureError::usage_code(
            "choosing parent tasks with - ^^ is not available in this build \
yet",
            "move_choice_unavailable",
        ));
    }
    // The grammar rejects a Create `#pomodoro` name with no ` :` task
    // before planning; this guards planners that bypass it. Offer defers
    // the check until after the pre-pick render, so a name is valid when
    // the note has an open task to offer.
    if form == ProjectNoteForm::Create
        && let Some(name) = pomodoro_name.as_deref()
    {
        let has_link = parsed.sub_bullets.iter().any(|sub_bullet| {
            sub_bullet.task_id.as_ref().is_some_and(|task| task.link)
                || sub_bullet
                    .move_task
                    .as_ref()
                    .is_some_and(|movement| movement.sigil == ':')
                || sub_bullet.move_choice.is_some()
        });
        if !has_link {
            return Err(CaptureError::usage(
                unused_project_note_pomodoro_error(name),
            ));
        }
    }
    let route = parsed.route.clone().ok_or_else(|| {
        CaptureError::io(
            "project-note capture invariant failed: route is missing",
        )
    })?;
    reject_project_note_conflicts(&parsed, request)?;

    let created = date_string(today);
    let priority = match parsed.priority_level {
        Some(number) => Some(resolve_priority(
            number,
            parsed.scheduled_offset,
            item_roll_seed(roll_seed, item_index),
            None,
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
    let schedule_log = schedule_log_reason.and_then(|reason| {
        scheduled.as_deref().map(|scheduled| {
            capture_schedule_log::plan("\t", scheduled, reason)
        })
    });

    validate_project_note_parent(planner, &request.bob_dir, &route)?;

    let basename =
        capture_project_note::project_note_basename(&route, &block_id);
    let source_path = request.bob_dir.join(format!("{route}.md"));
    let resolved_moves = resolve_project_note_moves(
        planner,
        &source_path,
        &route,
        &basename,
        &parsed.sub_bullets,
        scheduled.as_deref(),
        today,
        &request.bob_dir,
    )?;
    let move_hint = if resolved_moves.is_empty() {
        project_note_move_hint(
            planner,
            &source_path,
            scheduled.as_deref(),
            today,
            &request.bob_dir,
        )
    } else {
        None
    };
    let new_target = request.bob_dir.join(&basename);
    if planner.currently_exists(&new_target)? {
        return Err(CaptureError::io(format!(
            "project note already exists: {}",
            new_target.display()
        )));
    }
    validate_target_parent(&new_target)?;

    let schedule_log_lines: Vec<String> = schedule_log
        .as_ref()
        .map(|log| log.lines.clone())
        .unwrap_or_default();
    let settings = note_tasks::read_settings(&request.bob_dir);
    // Shortcuts already carry their ALL-CAPS title from the lexer. The
    // project-note writer converts those titles into `##` sections, merges
    // repeats, and keeps structural headings such as TASKS.
    let mut rendered = capture_project_note::render_project_note(
        &capture_project_note::ProjectNoteRenderInput {
            route: &route,
            block_id: &block_id,
            body: &parsed.body,
            now,
            scheduled: scheduled.as_deref(),
            priority: priority_field,
            schedule_log_lines: &schedule_log_lines,
            sub_bullets: &parsed.sub_bullets,
            status_types: settings.status_types.clone(),
        },
    );

    // A future project date blocks the whole note (§13.3, PDG5):
    // `:` tasks render `[?]` plus the tag and stay unlinked, with a
    // `not_linked` report. Only the unblocked `:` tasks link below.
    let project_blocked = scheduled
        .as_deref()
        .is_some_and(|date| date > date_string(today).as_str());

    // Inherited blocking at the new project boundary: when the staged
    // parent project is itself blocked (future schedule or an existing
    // #prj_is_blocked proxy from reconcile), the new note inherits that
    // state before links/offers. Keep the parent's reasons on its own
    // ^prj; do not copy dates/prerequisites or run vault-wide reconcile.
    // The child ^prj and open residents carry the Blocked/tag proxy so
    // the existing guard sees an unblocked destination as blocked.
    let inherited_blocked =
        parent_project_blocks_child(planner, &source_path, today, &settings);
    if inherited_blocked {
        apply_inherited_block_to_rendered(&mut rendered, &settings);
    }
    // Offers and bulk new-task links see the effective blocked state;
    // moved colon tasks still run the nested gesture so guard/consumption
    // and dependency-drop details stay truthful.
    let offer_blocked = project_blocked || inherited_blocked;

    let mut frozen_offers: Vec<FrozenOffer> = Vec::new();
    let mut project_folds: Vec<crate::native::task_complete::ProjectFold> =
        Vec::new();
    if form == ProjectNoteForm::Offer {
        let mut already_planned =
            rendered.task_rows.iter().filter(|row| row.link).count();
        let mut hidden = rendered
            .task_rows
            .iter()
            .filter(|row| !row.placeholder && row.hidden && !row.link)
            .count();
        let offer_rows: Vec<&capture_project_note::RenderedTask> =
            if offer_blocked {
                Vec::new()
            } else {
                rendered
                    .task_rows
                    .iter()
                    .filter(|row| {
                        is_offerable_row(
                            row,
                            &settings.status_types,
                            &request.bob_dir,
                        )
                    })
                    .collect()
            };
        // Moved candidates for Offer (before move): colon moves count as
        // planned, already-live-linked caret moves fold into planned,
        // hidden caret moves fold into hidden, Blocked caret moves are
        // excluded, and the rest are offered with `moved_from`.
        let mut moved_already_extra: usize = 0;
        let mut moved_hidden_extra: usize = 0;
        let mut moved_eligible_indices: Vec<usize> = Vec::new();
        let mut parent_all_ids: Vec<String> = Vec::new();
        if form == ProjectNoteForm::Offer && !resolved_moves.is_empty() {
            if let Some(parent_contents) = planner.peek_text(&source_path) {
                let parent_scan = note_tasks::scan(&parent_contents, &settings);
                parent_all_ids = parent_scan
                    .tasks()
                    .iter()
                    .filter_map(|task| task.block_id.clone())
                    .collect();
                let day_path = pomodoro::day_file_for(&request.bob_dir);
                // Staged-aware ledger: load the day only when offers need
                // it, so a never-loaded disk day agrees with the same day
                // already staged by an earlier item. No daily-note
                // requirement when there are no offer destinations.
                let day_contents =
                    planner.current_contents(&day_path).ok().flatten();
                let day_ledger = day_contents
                    .as_deref()
                    .map(capture_active_tasks::ledger_from_contents);
                let parent_lines: Vec<&str> = parent_contents.lines().collect();
                for (pos, resolved) in resolved_moves.iter().enumerate() {
                    if resolved.sigil == ':' {
                        moved_already_extra += 1;
                        continue;
                    }
                    if offer_blocked {
                        continue;
                    }
                    // Already-live-linked caret moves fold into planned.
                    // Exact dedicated-link identity: prose mentions,
                    // historical/closed entries, fenced/struck/deeper-nested
                    // rows, and prefix IDs (ab vs a) never count.
                    let live_key =
                        (route.to_string(), resolved.block_id.clone());
                    if day_ledger.as_ref().is_some_and(|ledger| {
                        ledger.owners.contains_key(&live_key)
                    }) {
                        moved_already_extra += 1;
                        continue;
                    }
                    // Hidden rows fold into hidden (muted, still movable).
                    // MovableRoot lines are 1-based; parent_lines is 0-based.
                    let hidden = parent_lines
                        .get(resolved.root.line.saturating_sub(1))
                        .is_some_and(|line| {
                            crate::native::capture_dependency_tasks::has_hide_tag(
                                line,
                            )
                        });
                    if hidden {
                        moved_hidden_extra += 1;
                        continue;
                    }
                    // Blocked rows are excluded (like new Blocked rows).
                    let status_name = task_complete::status_name_for(
                        &request.bob_dir,
                        resolved.root.status,
                    );
                    if status_name.eq_ignore_ascii_case("blocked") {
                        continue;
                    }
                    moved_eligible_indices.push(pos);
                }
            } else {
                // Parent disappeared after resolve; resolve would have
                // refused already, but keep eligible empty to avoid offering.
            }
        }
        already_planned += moved_already_extra;
        hidden += moved_hidden_extra;
        let has_colon = parsed.sub_bullets.iter().any(|sub_bullet| {
            sub_bullet.task_id.as_ref().is_some_and(|task| task.link)
                || sub_bullet
                    .move_task
                    .as_ref()
                    .is_some_and(|movement| movement.sigil == ':')
        });
        if let Some(name) = pomodoro_name.as_deref()
            && offer_rows.is_empty()
            && moved_eligible_indices.is_empty()
            && !has_colon
        {
            return Err(CaptureError::usage(
                unused_project_note_offer_pomodoro_error(name),
            ));
        }
        let relative = std::path::PathBuf::from(&basename);
        for (n, row) in offer_rows.iter().enumerate() {
            let index = rendered
                .task_rows
                .iter()
                .position(|candidate| std::ptr::eq(*row, candidate))
                .unwrap_or(n);
            let ordinal = digest_ordinal_for(
                &rendered.task_rows,
                index,
                &settings.global_filter,
            );
            let key = note_tasks::stable_task_key(
                &relative,
                row.block_id.as_deref(),
                &row.text,
                &settings.global_filter,
                ordinal,
            );
            let selected = match &request.selection {
                crate::native::task_complete::SuccessorSelection::All => true,
                crate::native::task_complete::SuccessorSelection::None => false,
                crate::native::task_complete::SuccessorSelection::Keys(
                    keys,
                ) => keys.contains(&key),
            };
            frozen_offers.push(FrozenOffer {
                key,
                n: n + 1,
                order_key: (
                    crate::native::task_complete::OfferGroup::Project as u8,
                    (usize::MAX, usize::MAX),
                    0,
                    relative.clone(),
                    row.line,
                ),
                source_index: row.source_index,
                pre_status: row.status,
                text: row.text.clone(),
                selected,
                final_block_id: row.block_id.clone(),
                block_id_needed: row.block_id.is_none(),
                final_status: row.status,
                final_line: row.line,
                moved_from: None,
                settled_ordinal: None,
            });
        }
        // Moved offers for Offer+moves (before move): eligible caret moves
        // are offered with `moved_from`, keys frozen as `<new>#^<id>`.
        // Colon moves and already-live-linked/hidden/Blocked moves are not
        // offered (they count in already_planned/hidden above).
        if !moved_eligible_indices.is_empty() {
            let moved_from_label = format!("{route}.md");
            for pos in &moved_eligible_indices {
                let resolved = &resolved_moves[*pos];
                let key = note_tasks::stable_task_key(
                    &relative,
                    Some(resolved.block_id.as_str()),
                    &resolved.root.title,
                    &settings.global_filter,
                    1,
                );
                let selected = match &request.selection {
                    crate::native::task_complete::SuccessorSelection::All => {
                        true
                    }
                    crate::native::task_complete::SuccessorSelection::None => {
                        false
                    }
                    crate::native::task_complete::SuccessorSelection::Keys(
                        keys,
                    ) => keys.contains(&key),
                };
                frozen_offers.push(FrozenOffer {
                    key,
                    n: 0,
                    order_key: (
                        crate::native::task_complete::OfferGroup::Project as u8,
                        (usize::MAX, usize::MAX),
                        0,
                        relative.clone(),
                        resolved.source_index,
                    ),
                    source_index: Some(resolved.source_index),
                    pre_status: resolved.root.status,
                    text: resolved.root.title.clone(),
                    selected,
                    final_block_id: Some(resolved.block_id.clone()),
                    block_id_needed: false,
                    final_status: resolved.root.status,
                    final_line: 0,
                    moved_from: Some(moved_from_label.clone()),
                    settled_ordinal: None,
                });
            }
            // Draft-order numbering for Offer+moves: new and moved offers
            // share one number space in source order (final-note order).
            // New-only drafts keep their row.line order above.
            frozen_offers
                .sort_by_key(|offer| offer.source_index.unwrap_or(usize::MAX));
            for (idx, offer) in frozen_offers.iter_mut().enumerate() {
                offer.n = idx + 1;
                if offer.moved_from.is_none() {
                    // New offers: use source order for global numbering when
                    // moves exist, preserving row.line order otherwise.
                    if let Some(source_index) = offer.source_index {
                        offer.order_key = (
                            crate::native::task_complete::OfferGroup::Project
                                as u8,
                            (usize::MAX, usize::MAX),
                            0,
                            relative.clone(),
                            source_index,
                        );
                    }
                }
            }
        }
        if frozen_offers.iter().any(|offer| offer.selected) {
            let mut reserved: Vec<String> = parsed
                .sub_bullets
                .iter()
                .filter_map(|bullet| {
                    bullet.task_id.as_ref().map(|task| task.block_id.clone())
                })
                .collect();
            reserved.push("prj".to_string());
            // Moved and descendant IDs must never collide with minted IDs
            // for selected ID-less authored tasks.
            for resolved in resolved_moves.iter() {
                reserved.push(resolved.block_id.clone());
            }
            reserved.extend(parent_all_ids.iter().cloned());
            for offer in frozen_offers
                .iter_mut()
                .filter(|offer| offer.selected && offer.moved_from.is_none())
            {
                let Some(source_index) = offer.source_index else {
                    continue;
                };
                let Some(bullet) = parsed.sub_bullets.get_mut(source_index)
                else {
                    continue;
                };
                match bullet.task_id.as_mut() {
                    Some(task) => {
                        task.link = true;
                        offer.final_block_id = Some(task.block_id.clone());
                    }
                    None => {
                        let used: std::collections::HashSet<&str> =
                            reserved.iter().map(String::as_str).collect();
                        let minted =
                            crate::native::capture_block_ids::mint_block_id(
                                &offer.text,
                                &used,
                            );
                        reserved.push(minted.clone());
                        bullet.task_id = Some(
                            crate::native::capture_language::ProjectTaskId {
                                block_id: minted.clone(),
                                link: true,
                            },
                        );
                        offer.final_block_id = Some(minted);
                    }
                }
            }
            rendered = capture_project_note::render_project_note(
                &capture_project_note::ProjectNoteRenderInput {
                    route: &route,
                    block_id: &block_id,
                    body: &parsed.body,
                    now,
                    scheduled: scheduled.as_deref(),
                    priority: priority_field,
                    schedule_log_lines: &schedule_log_lines,
                    sub_bullets: &parsed.sub_bullets,
                    status_types: settings.status_types.clone(),
                },
            );
            for offer in &mut frozen_offers {
                let Some(source_index) = offer.source_index else {
                    continue;
                };
                if let Some(row) = rendered
                    .task_rows
                    .iter()
                    .find(|row| row.source_index == Some(source_index))
                {
                    offer.final_status = row.status;
                    offer.final_line = row.line;
                    if offer.final_block_id.is_none() {
                        offer.final_block_id = row.block_id.clone();
                    }
                }
            }
        }
        // The identity the batch net pass resolves unselected ID-less
        // rows by. The picks above have already minted their IDs, so the
        // pre-pick ordinal frozen into each key no longer addresses the
        // surviving ID-less population.
        for offer in &mut frozen_offers {
            offer.settled_ordinal = settled_ordinal_for(
                &rendered.task_rows,
                offer.source_index,
                &settings.global_filter,
            );
        }
        project_folds.push(crate::native::task_complete::ProjectFold {
            project: relative,
            hidden,
            already_planned,
            sub_projects: 0,
        });
    }

    let mut move_execution = if resolved_moves.is_empty() {
        None
    } else {
        Some(execute_project_note_moves(
            request,
            planner,
            &source_path,
            &new_target,
            &route,
            &basename,
            &rendered,
            &resolved_moves,
            &settings,
            today,
            item_index + 1,
        )?)
    };

    // Moved offers: final status/line from landed rows after the move
    // (before any linking, which re-resolves by ID afterwards).
    if form == ProjectNoteForm::Offer
        && let Some(execution) = move_execution.as_ref()
    {
        for offer in frozen_offers
            .iter_mut()
            .filter(|offer| offer.moved_from.is_some())
        {
            if let Some(block_id) = offer.final_block_id.clone()
                && let Some(moved) = execution
                    .moved_tasks
                    .iter()
                    .find(|task| task.block_id == block_id)
            {
                offer.final_status = moved.status_symbol;
                offer.final_line = moved.line;
                offer.pre_status = moved.previous_status_symbol;
                offer.text = moved.text.clone();
            }
        }
    }

    let has_colon_move = resolved_moves.iter().any(|m| m.sigil == ':');
    let project_link_guard: Option<LinkGuardJson> = if project_blocked
        && (form == ProjectNoteForm::Offer
            || rendered.named_tasks.iter().any(|task| task.link)
            || has_colon_move)
    {
        Some(LinkGuardJson {
            reasons: vec!["project_schedule".to_string()],
            promoted: Vec::new(),
            detail: Some(format!(
                "not linked (project scheduled {})",
                scheduled.as_deref().unwrap_or_default(),
            )),
            drop_refused: None,
            removed_pomodoro_links: None,
            day_file: None,
        })
    } else {
        None
    };
    // One Task Link per `:` task, in source order. With no links the daily
    // note is never read; otherwise the note and the day file are staged
    // together so any failure writes nothing. Offer also reads the day
    // when it has eligible rows, so unselected destinations can reuse
    // the same placement helper on scratch text.
    let stem = rendered
        .basename
        .strip_suffix(".md")
        .unwrap_or(rendered.basename.as_str())
        .to_string();
    let linked_all: Vec<capture_project_note::RenderedNamedTask> = rendered
        .named_tasks
        .iter()
        .filter(|task| task.link && !offer_blocked)
        .cloned()
        .collect();
    // For Create with colon moves, and for Offer with any moves, the bulk
    // new-task path below is skipped (it would link new tasks before moved
    // ones). The draft-ordered pass after it links new and moved tasks
    // together in source order.
    let use_draft_ordered_links = !project_blocked
        && ((form == ProjectNoteForm::Create && has_colon_move)
            || (form == ProjectNoteForm::Offer && !resolved_moves.is_empty()));
    let linked: Vec<capture_project_note::RenderedNamedTask> =
        if use_draft_ordered_links {
            Vec::new()
        } else {
            linked_all.clone()
        };
    let note_route: &str = basename.strip_suffix(".md").unwrap_or(&basename);
    // Stage the new note before linking so per-task type resolution (D3)
    // sees the staged residence, including the just-rendered task lines.
    if move_execution.is_none() {
        planner.stage(&new_target, rendered.contents.clone())?;
    }
    let need_moved_day = !project_blocked
        && move_execution.as_ref().is_some_and(|execution| {
            execution.moved_tasks.iter().any(|task| {
                // Colon moves link; selected moved offers (Offer, caret
                // selected) link via the same operation (tracked through
                // frozen offers, but need_day already covers offers).
                task.sigil == ':' && task.planned.is_none()
            })
        });
    let need_day =
        !linked.is_empty() || !frozen_offers.is_empty() || need_moved_day;
    let mut actual_placements: Vec<PlacedProjectLink> = Vec::new();
    let mut scratch_placements: Vec<PlacedProjectLink> = Vec::new();
    let mut extra_pomodoro_refs: Vec<super::PomodoroBlockRef> = Vec::new();
    let mut extra_task_block_refs: Vec<super::TaskBlockRef> = Vec::new();
    let mut extra_dropped: Vec<super::output::DroppedDependencyJson> =
        Vec::new();
    let mut extra_blocks_updates: Vec<super::output::BlocksLineUpdateJson> =
        Vec::new();
    // Successor-link info for selected moved offers (Offer+moves,
    // draft-ordered): block ID → (entry name, entry line, created, next_up).
    let mut moved_selected_successor: std::collections::HashMap<
        String,
        (String, usize, bool, bool),
    > = std::collections::HashMap::new();
    let (
        mut day_file,
        mut pomodoro_link_placement,
        mut canonical_pomodoro_name,
        mut creates_pomodoro,
        mut first_link_destination,
        mut day_relative,
    ) = if !need_day {
        (None, None, None, None, None, None)
    } else {
        let day_file = pomodoro::day_file_for(&request.bob_dir);
        if !planner.currently_exists(&day_file)? {
            return Err(CaptureError::io(format!(
                "Bob daily note does not exist: {}",
                day_file.display()
            )));
        }
        if paths_refer_to_same_file(&new_target, &day_file) {
            return Err(CaptureError::io(
                "routed note and Bob daily note must be different files",
            ));
        }
        let original_day = planner.read_existing(&day_file)?;
        let scan = capture_pomodoros::scan(&original_day);
        let (canonical_pomodoro_name, mut creates_pomodoro) =
            match pomodoro_name.as_deref() {
                Some(selector) => {
                    // A literal's canonical name is final; a plain selector
                    // reuses a completed entry's spelling.
                    let canonical_fallback = if pomodoro_name_is_literal {
                        capture_pomodoros::canonicalize_exact_name(selector)
                    } else {
                        capture_pomodoros::canonicalize_pomodoro_name(selector)
                    }
                    .ok_or_else(|| {
                        CaptureError::usage(
                            capture_pomodoros::POMODORO_NAME_USAGE,
                        )
                    })?;
                    match capture_pomodoros::select_capture_name(
                        &scan,
                        selector,
                        pomodoro_name_is_literal,
                    ) {
                        capture_pomodoros::NamedSelection::Found(entry) => (
                            Some(
                                entry
                                    .name
                                    .clone()
                                    .unwrap_or(canonical_fallback),
                            ),
                            false,
                        ),
                        capture_pomodoros::NamedSelection::CompletedOnly(
                            entry,
                        ) => (
                            Some(
                                entry
                                    .name
                                    .clone()
                                    .unwrap_or(canonical_fallback),
                            ),
                            true,
                        ),
                        capture_pomodoros::NamedSelection::Missing {
                            ..
                        } => (Some(canonical_fallback), true),
                    }
                }
                None => (None, false),
            };
        let (mut day, placements) = place_project_task_links(
            original_day,
            &linked,
            &stem,
            note_route,
            pomodoro_name.as_deref(),
            pomodoro_name_is_literal,
            planner,
            task_theme_context,
            warnings,
            true,
        )?;
        let first_placement =
            placements.first().map(|placed| placed.insertion.placement);
        let first_destination = placements.first().and_then(|placed| {
            placed
                .insertion
                .destination
                .placed_type
                .clone()
                .map(|typed| {
                    let mut destination = placed.insertion.destination.clone();
                    destination.placed_type = Some(typed);
                    destination
                })
        });
        for placed in &placements {
            creates_pomodoro =
                creates_pomodoro || placed.insertion.creates_pomodoro;
        }
        if !linked.is_empty() {
            planner.stage(&day_file, day.clone())?;
        } else {
            day = planner.read_existing(&day_file)?;
        }
        let mut probe_reserved: Vec<String> = parsed
            .sub_bullets
            .iter()
            .filter_map(|bullet| {
                bullet.task_id.as_ref().map(|task| task.block_id.clone())
            })
            .collect();
        probe_reserved.push("prj".to_string());
        probe_reserved.extend(
            frozen_offers
                .iter()
                .filter_map(|offer| offer.final_block_id.clone()),
        );
        let unselected: Vec<capture_project_note::RenderedNamedTask> =
            frozen_offers
                .iter()
                .filter(|offer| !offer.selected)
                .filter_map(|offer| {
                    let row = rendered
                        .task_rows
                        .iter()
                        .find(|row| row.source_index == offer.source_index)?;
                    let block_id = if let Some(id) =
                        offer.final_block_id.as_deref()
                    {
                        id.to_string()
                    } else {
                        let used: std::collections::HashSet<&str> =
                            probe_reserved.iter().map(String::as_str).collect();
                        let minted =
                            crate::native::capture_block_ids::mint_block_id(
                                &offer.text,
                                &used,
                            );
                        probe_reserved.push(minted.clone());
                        minted
                    };
                    Some(named_task_for_offer(row, &block_id))
                })
                .collect();
        let (_, scratch) = place_project_task_links(
            day,
            &unselected,
            &stem,
            note_route,
            pomodoro_name.as_deref(),
            pomodoro_name_is_literal,
            planner,
            task_theme_context,
            warnings,
            false,
        )?;
        scratch_placements = scratch;
        actual_placements = placements;
        let day_relative = Some(capture_pomodoros::relative_day_file(
            &day_file,
            &request.bob_dir,
        ));
        (
            if linked.is_empty() {
                None
            } else {
                Some(day_file.display().to_string())
            },
            first_placement,
            canonical_pomodoro_name,
            if linked.is_empty() {
                None
            } else {
                Some(creates_pomodoro)
            },
            first_destination,
            day_relative,
        )
    };

    let mut task_links = linked
        .iter()
        .map(|task| ProjectTaskLinkJson {
            block_id: task.block_id.clone(),
            block_link: format!(
                "[[{stem}#^{block_id}]]",
                block_id = task.block_id
            ),
            text: task.text.clone(),
            task_line: task.task_line.clone(),
        })
        .collect::<Vec<_>>();
    let mut task_theme = None;
    let mut project_merges: Vec<super::task_theme::AppliedMerge> = Vec::new();
    if project_link_guard.is_none()
        && !inherited_blocked
        && let Some(initial_destination) = canonical_pomodoro_name.clone()
        && let Some(selector) = pomodoro_name.as_deref()
    {
        let mut current_destination = initial_destination;
        for task in &linked {
            if let Some(applied) = task_theme_context.apply_new_task(
                planner,
                &new_target,
                note_route,
                &task.block_id,
                Some(current_destination.as_str()),
                Some(selector),
                warnings,
            )? {
                if let Some(link) = task_links
                    .iter_mut()
                    .find(|link| link.block_id == task.block_id)
                {
                    link.task_line = applied.task_line.clone();
                }
                if task_theme.is_none() {
                    task_theme = applied.theme;
                }
                if let Some(merge) = applied.merge {
                    current_destination = merge.after.clone();
                    project_merges.push(merge);
                }
            }
        }
        if !project_merges.is_empty() {
            // The destination gained each missing type once, in link order.
            let before = project_merges[0].before.clone();
            let after = project_merges[project_merges.len() - 1].after.clone();
            let added = project_merges
                .iter()
                .map(|merge| merge.added.clone())
                .collect::<Vec<_>>();
            canonical_pomodoro_name = Some(after.clone());
            if let Some(destination) = first_link_destination.as_mut() {
                destination.name = Some(after.clone());
                destination.merged = Some(super::batch::MergedPomodoro {
                    before,
                    after,
                    added,
                });
            }
        }
    }

    // Blocked-at-birth colon moves stay unlinked with a project_schedule
    // reason; they already moved (when eligible) and need no day file.
    if project_blocked && let Some(execution) = move_execution.as_mut() {
        for task in execution.moved_tasks.iter_mut().filter(|task| {
            task.sigil == ':' && task.planned.is_none() && task.link.is_none()
        }) {
            task.not_linked = Some("project_schedule".to_string());
            task.detail = super::move_consequence::describe(
                &super::move_consequence::Outcome {
                    sigil: task.sigil,
                    previous_status: task.previous_status_symbol,
                    status: task.status_symbol,
                    existing_today_link: None,
                    added_today_link: None,
                    own_scheduled: None,
                    project_starts: scheduled.as_deref().map(display_move_date),
                    guard: super::move_consequence::LinkGuard::Link,
                    id_added: None,
                },
            );
        }
    }

    // Draft-ordered links for Create with colon moves: new `:` tasks and
    // colon moves link in source order through their respective gestures.
    // The bulk path above was skipped (linked emptied) so placements do not
    // run new-first; this pass preserves draft order for ledger, task_links,
    // merges, and reporting.
    if use_draft_ordered_links && let Some(execution) = move_execution.as_mut()
    {
        use crate::native::capture_task_toggle::PomodoroLinkTarget;

        let day_path = pomodoro::day_file_for(&request.bob_dir);
        // Bulk with empty linked already verified existence when need_day;
        // re-check for the draft-ordered pass which stages day itself.
        if !planner.currently_exists(&day_path)? {
            return Err(CaptureError::io(format!(
                "Bob daily note does not exist: {}",
                day_path.display()
            )));
        }
        if paths_refer_to_same_file(&new_target, &day_path) {
            return Err(CaptureError::io(
                "routed note and Bob daily note must be different files",
            ));
        }
        let mut day = planner.read_existing(&day_path)?;
        let mut current_destination = canonical_pomodoro_name.clone();
        let mut first_link_seen = first_link_destination.is_some();
        let mut day_file_set = day_file.is_some();
        let mut creates_bool = creates_pomodoro.unwrap_or(false);

        // Map new tasks by block ID for draft-order lookup.
        let new_by_id: std::collections::HashMap<
            &str,
            &capture_project_note::RenderedNamedTask,
        > = linked_all
            .iter()
            .map(|task| (task.block_id.as_str(), task))
            .collect();
        // Map moves that link in draft order: colon moves (both forms)
        // plus selected moved offers (Offer caret selected). Unselected
        // caret moves, already-live-linked moves, hidden/Blocked moves,
        // and project-blocked moves never link here.
        let mut moved_link_by_source: std::collections::HashMap<
            usize,
            &ResolvedProjectMove,
        > = resolved_moves
            .iter()
            .filter(|m| m.sigil == ':')
            .map(|m| (m.source_index, m))
            .collect();
        if form == ProjectNoteForm::Offer {
            for offer in frozen_offers
                .iter()
                .filter(|offer| offer.selected && offer.moved_from.is_some())
            {
                if let Some(source_index) = offer.source_index
                    && let Some(resolved) = resolved_moves
                        .iter()
                        .find(|m| m.source_index == source_index)
                {
                    moved_link_by_source.insert(source_index, resolved);
                }
            }
        }
        // Map moved_tasks entries by block ID for updates.
        let mut moved_index_by_id: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for (idx, task) in execution.moved_tasks.iter().enumerate() {
            moved_index_by_id.insert(task.block_id.clone(), idx);
        }

        for (source_index, bullet) in parsed.sub_bullets.iter().enumerate() {
            // New `:` task in draft order. Inherited-blocked projects stay
            // unlinked like future-dated ones; the staged [?] + tag rows
            // already carry the proxy.
            if let Some(task_id) = bullet.task_id.as_ref()
                && task_id.link
                && let Some(named) = new_by_id.get(task_id.block_id.as_str())
            {
                if inherited_blocked {
                    continue;
                }
                let named = (*named).clone();
                // Per-task type placement when no `#name` (D3); named keeps
                // its deliberate destination (the caller applies D6 below).
                let placement_type = if pomodoro_name.is_none() {
                    task_theme_context.placement_for_task(
                        planner,
                        note_route,
                        &named.task_line,
                        warnings,
                    )
                } else {
                    None
                };
                let typed_pair = placement_type.as_ref().map(|placement| {
                    (placement.as_typed(), placement.placed_type())
                });
                let (typed, placed_type) = match typed_pair.as_ref() {
                    Some((typed, placed)) => {
                        (Some(typed), Some(placed.clone()))
                    }
                    None => (None, None),
                };
                let mut insertion = insert_pomodoro_block_link(
                    &day,
                    &format!(
                        "[[{stem}#^{block_id}]]",
                        block_id = named.block_id
                    ),
                    pomodoro_name.as_deref(),
                    pomodoro_name_is_literal,
                    typed,
                )?;
                if let Some(placed_type) = placed_type {
                    insertion.destination.placed_type = Some(placed_type);
                }
                day = insertion.updated.clone();
                planner.stage(&day_path, day.clone())?;
                creates_bool = creates_bool || insertion.creates_pomodoro;
                if !first_link_seen {
                    pomodoro_link_placement = Some(insertion.placement);
                    let mut dest = insertion.destination.clone();
                    if let Some(placed) = dest.placed_type.clone() {
                        dest.placed_type = Some(placed);
                    }
                    first_link_destination = Some(dest);
                    first_link_seen = true;
                }
                if !day_file_set {
                    day_file = Some(day_path.display().to_string());
                    day_file_set = true;
                }
                // Task link entry in draft order; task_line updated below
                // when typing applies.
                task_links.push(ProjectTaskLinkJson {
                    block_id: named.block_id.clone(),
                    block_link: format!(
                        "[[{stem}#^{block_id}]]",
                        block_id = named.block_id
                    ),
                    text: named.text.clone(),
                    task_line: named.task_line.clone(),
                });
                // Actual placement for Offer selected new offers (successors
                // preview/link). Explicit `:` tasks and Create need none.
                if form == ProjectNoteForm::Offer
                    && frozen_offers.iter().any(|offer| {
                        offer.selected
                            && offer.moved_from.is_none()
                            && offer.final_block_id.as_deref()
                                == Some(named.block_id.as_str())
                    })
                {
                    actual_placements.push(PlacedProjectLink {
                        task: named.clone(),
                        insertion: PomodoroBlockLinkInsertion {
                            updated: insertion.updated.clone(),
                            placement: insertion.placement,
                            destination: insertion.destination.clone(),
                            creates_pomodoro: insertion.creates_pomodoro,
                        },
                        used_typed: typed.is_some(),
                    });
                }
                // Deliberate named-type behavior (D6) for new tasks.
                if project_link_guard.is_none()
                    && let Some(dest_name) = current_destination.clone()
                    && let Some(selector) = pomodoro_name.as_deref()
                    && let Some(applied) = task_theme_context.apply_new_task(
                        planner,
                        &new_target,
                        note_route,
                        &named.block_id,
                        Some(dest_name.as_str()),
                        Some(selector),
                        warnings,
                    )?
                {
                    if let Some(link) = task_links
                        .iter_mut()
                        .find(|link| link.block_id == named.block_id)
                    {
                        link.task_line = applied.task_line.clone();
                    }
                    if task_theme.is_none() {
                        task_theme = applied.theme;
                    }
                    if let Some(merge) = applied.merge {
                        current_destination = Some(merge.after.clone());
                        project_merges.push(merge);
                    }
                }
                continue;
            }
            // Moved link in draft order: colon moves plus selected moved
            // offers (Offer). Unselected, already-live-linked, hidden and
            // Blocked moves never reach here.
            if let Some(resolved) = moved_link_by_source.get(&source_index) {
                let block_id = resolved.block_id.clone();
                let Some(moved_idx) = moved_index_by_id.get(&block_id).copied()
                else {
                    continue;
                };
                // Followed live link stays; no new link, no relocation.
                if execution.moved_tasks[moved_idx].planned.is_some() {
                    continue;
                }
                // Project already blocks; handled above, skip day work.
                if project_blocked {
                    continue;
                }
                let ledger_target = match pomodoro_name.as_deref() {
                    Some(name) if pomodoro_name_is_literal => {
                        PomodoroLinkTarget::LiteralName(name)
                    }
                    Some(name) => PomodoroLinkTarget::Named(name),
                    None => PomodoroLinkTarget::Implicit,
                };
                let nested = plan_existing_task_pomodoro_link_item(
                    planner,
                    &request.bob_dir,
                    request.dry_run,
                    &new_target,
                    note_route,
                    &block_id,
                    ledger_target,
                    None,
                    now,
                    today,
                    warnings,
                    dependency_ctx,
                    Some(task_theme_context),
                    false,
                )?;
                // Aggregate block/dependency tracking for the batch.
                extra_pomodoro_refs
                    .extend(nested.pomodoro_refs.iter().cloned());
                extra_task_block_refs
                    .extend(nested.task_block_refs.iter().cloned());
                extra_dropped
                    .extend(nested.result.dropped_dependencies.iter().cloned());
                extra_blocks_updates
                    .extend(nested.result.blocks_line_updates.iter().cloned());
                if let Some(guard) = nested.result.link_guard.clone() {
                    let reason = guard
                        .reasons
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "blocked".to_string());
                    let entry = &mut execution.moved_tasks[moved_idx];
                    entry.not_linked = Some(reason);
                    entry.detail = super::move_consequence::describe(
                        &super::move_consequence::Outcome {
                            sigil: entry.sigil,
                            previous_status: entry.status_symbol,
                            status: entry.status_symbol,
                            existing_today_link: None,
                            added_today_link: None,
                            own_scheduled: None,
                            project_starts: None,
                            guard:
                                super::move_consequence::LinkGuard::Consume {
                                    reason: guard
                                        .detail
                                        .as_deref()
                                        .map(move_guard_reason)
                                        .unwrap_or_else(|| {
                                            "blocked".to_string()
                                        }),
                                },
                            id_added: None,
                        },
                    );
                    // Sync day for subsequent ops; consumption may have
                    // staged day changes even without a link.
                    if let Ok(current) = planner.read_existing(&day_path) {
                        day = current;
                    }
                    continue;
                }
                // Link succeeded; deliberate named retype (plan.rs outer).
                let mut nested_result = nested.result;
                if pomodoro_name.is_some()
                    && nested_result.link_guard.is_none()
                    && let Some(previous) =
                        nested_result.previous_task_line.clone()
                    && let Some(resolved_name) = nested_result
                        .pomodoro_link_destination
                        .as_ref()
                        .and_then(|dest| dest.name.clone())
                    && let Some(selector) = pomodoro_name.as_deref()
                    && let Some(applied) = task_theme_context
                        .apply_existing_task(
                            planner,
                            &new_target,
                            note_route,
                            &block_id,
                            &previous,
                            Some(resolved_name.as_str()),
                            Some(selector),
                            warnings,
                        )?
                {
                    nested_result.task_line = applied.task_line.clone();
                    if task_theme.is_none() {
                        task_theme = applied.theme;
                    }
                    if let Some(merge) = applied.merge {
                        current_destination = Some(merge.after.clone());
                        project_merges.push(merge);
                    }
                }
                creates_bool = creates_bool
                    || nested_result.creates_pomodoro.unwrap_or(false);
                if !first_link_seen {
                    pomodoro_link_placement =
                        nested_result.pomodoro_link_placement;
                    first_link_destination =
                        nested_result.pomodoro_link_destination.clone();
                    first_link_seen = true;
                }
                if !day_file_set {
                    day_file = Some(day_path.display().to_string());
                    day_file_set = true;
                }
                // Re-resolve final moved row by ID after mutations.
                let final_contents = planner.read_existing(&new_target)?;
                let final_scan = note_tasks::scan(&final_contents, &settings);
                let (final_status, final_line, final_task_line) =
                    match final_scan.by_block_id(&block_id) {
                        BlockIdLookup::Found(task) => {
                            let line = final_contents
                                .lines()
                                .nth(task.line_index)
                                .unwrap_or_default()
                                .to_string();
                            (task.status_symbol, task.line_index + 1, line)
                        }
                        _ => {
                            return Err(CaptureError::io(format!(
                                "capture move lost block ^{block_id} in {basename} after linking"
                            )));
                        }
                    };
                let final_status_name = task_complete::status_name_for(
                    &request.bob_dir,
                    final_status,
                );
                let previous_status =
                    execution.moved_tasks[moved_idx].status_symbol;
                // Detail suffix copy (without the `Moves in` prefix).
                let entry_name = nested_result
                    .pomodoro_link_destination
                    .as_ref()
                    .and_then(|dest| dest.name.clone())
                    .unwrap_or_default();
                let dropped_prerequisites =
                    nested_result.dropped_dependencies.len();
                let pulled_forward = nested_result
                    .previous_task_line
                    .as_deref()
                    .and_then(scheduled_field)
                    .filter(|date| {
                        scheduled_field(&final_task_line).as_ref() != Some(date)
                    });
                let detail = super::move_consequence::describe(
                    &super::move_consequence::Outcome {
                        sigil: execution.moved_tasks[moved_idx].sigil,
                        previous_status,
                        status: final_status,
                        existing_today_link: None,
                        added_today_link: Some(entry_name.clone()),
                        own_scheduled: None,
                        project_starts: None,
                        guard: if dropped_prerequisites > 0 {
                            super::move_consequence::LinkGuard::DropPrerequisites {
                                count: dropped_prerequisites,
                                pulled_forward,
                            }
                        } else if let Some(date) = pulled_forward {
                            super::move_consequence::LinkGuard::PullForward {
                                date,
                            }
                        } else {
                            super::move_consequence::LinkGuard::Link
                        },
                        id_added: None,
                    },
                );
                {
                    let entry = &mut execution.moved_tasks[moved_idx];
                    entry.status_symbol = final_status;
                    entry.status_name = final_status_name;
                    entry.line = final_line;
                    entry.link = Some(ProjectTaskLinkJson {
                        block_id: block_id.clone(),
                        block_link: format!("[[{stem}#^{block_id}]]"),
                        text: entry.text.clone(),
                        task_line: final_task_line.clone(),
                    });
                    entry.not_linked = None;
                    entry.detail = detail;
                }
                task_links.push(ProjectTaskLinkJson {
                    block_id: block_id.clone(),
                    block_link: format!("[[{stem}#^{block_id}]]"),
                    text: execution.moved_tasks[moved_idx].text.clone(),
                    task_line: final_task_line.clone(),
                });
                // Successor-link info for Offer selected moved offers
                // (picked rows appear twice: successors with link + task_links).
                if form == ProjectNoteForm::Offer
                    && frozen_offers.iter().any(|offer| {
                        offer.selected
                            && offer.moved_from.is_some()
                            && offer.final_block_id.as_deref()
                                == Some(block_id.as_str())
                    })
                    && let Some(destination) =
                        nested_result.pomodoro_link_destination.as_ref()
                {
                    let entry_name =
                        destination.name.clone().unwrap_or_default();
                    let entry_line = destination.line;
                    let entry_created =
                        nested_result.creates_pomodoro.unwrap_or(false);
                    let next_up = destination.role == Some("next_up");
                    moved_selected_successor.insert(
                        block_id.clone(),
                        (entry_name, entry_line, entry_created, next_up),
                    );
                }
                // Sync day for subsequent ops.
                if let Ok(current) = planner.read_existing(&day_path) {
                    day = current;
                }
            }
        }
        // Scratch previews for Offer unselected offers (new + moved) on the
        // final day after actual links (sequential placement after actual).
        // The bulk path computed new-only scratch on the original day before
        // actual links; recompute here in frozen-offers order (draft order)
        // for both new and moved unselected.
        if form == ProjectNoteForm::Offer
            && frozen_offers.iter().any(|offer| !offer.selected)
        {
            let mut probe_reserved: Vec<String> = parsed
                .sub_bullets
                .iter()
                .filter_map(|bullet| {
                    bullet.task_id.as_ref().map(|task| task.block_id.clone())
                })
                .collect();
            probe_reserved.push("prj".to_string());
            for offer in frozen_offers.iter() {
                if let Some(block_id) = offer.final_block_id.clone() {
                    probe_reserved.push(block_id);
                }
            }
            for resolved in resolved_moves.iter() {
                probe_reserved.push(resolved.block_id.clone());
            }
            // Final new-note contents after actual links for moved unselected
            // task lines and reservation of all IDs (including descendants).
            let final_new_contents =
                planner.read_existing(&new_target).unwrap_or_default();
            let final_new_scan =
                note_tasks::scan(&final_new_contents, &settings);
            for task in final_new_scan.tasks() {
                if let Some(block_id) = task.block_id.clone()
                    && !probe_reserved.contains(&block_id)
                {
                    probe_reserved.push(block_id);
                }
            }
            let mut unselected: Vec<capture_project_note::RenderedNamedTask> =
                Vec::new();
            for offer in frozen_offers.iter().filter(|offer| !offer.selected) {
                if offer.moved_from.is_none() {
                    // New unselected: rendered row + minted preview ID when
                    // ID-less (like the bulk path, but on final day).
                    let Some(source_index) = offer.source_index else {
                        continue;
                    };
                    let Some(row) = rendered
                        .task_rows
                        .iter()
                        .find(|row| row.source_index == Some(source_index))
                    else {
                        continue;
                    };
                    let mut block_id =
                        offer.final_block_id.clone().unwrap_or_default();
                    if block_id.is_empty() {
                        let used: std::collections::HashSet<&str> =
                            probe_reserved.iter().map(String::as_str).collect();
                        block_id =
                            crate::native::capture_block_ids::mint_block_id(
                                &row.text, &used,
                            );
                        probe_reserved.push(block_id.clone());
                    }
                    unselected.push(named_task_for_offer(row, &block_id));
                } else {
                    // Moved unselected: fake named task from landed row.
                    let Some(block_id) = offer.final_block_id.clone() else {
                        continue;
                    };
                    let task_line = match final_new_scan.by_block_id(&block_id)
                    {
                        BlockIdLookup::Found(task) => final_new_contents
                            .lines()
                            .nth(task.line_index)
                            .map(|line| line.to_string())
                            .unwrap_or_else(|| {
                                format!(
                                    "- [ ] #task {} ^{block_id}",
                                    offer.text
                                )
                            }),
                        _ => format!("- [ ] #task {} ^{block_id}", offer.text),
                    };
                    unselected.push(capture_project_note::RenderedNamedTask {
                        block_id: block_id.clone(),
                        link: true,
                        text: offer.text.clone(),
                        task_line,
                    });
                }
            }
            if !unselected.is_empty()
                && let Ok((_, scratch)) = place_project_task_links(
                    day.clone(),
                    &unselected,
                    &stem,
                    note_route,
                    pomodoro_name.as_deref(),
                    pomodoro_name_is_literal,
                    planner,
                    task_theme_context,
                    warnings,
                    false,
                )
            {
                scratch_placements = scratch;
            }
        }
        if first_link_seen {
            creates_pomodoro = Some(creates_bool);
            // Final canonical from the combined merge chain, mirroring the
            // new-task path above.
            if !project_merges.is_empty() {
                let before = project_merges[0].before.clone();
                let after =
                    project_merges[project_merges.len() - 1].after.clone();
                let added = project_merges
                    .iter()
                    .map(|merge| merge.added.clone())
                    .collect::<Vec<_>>();
                canonical_pomodoro_name = Some(after.clone());
                current_destination = canonical_pomodoro_name.clone();
                if let Some(destination) = first_link_destination.as_mut() {
                    destination.name = Some(after.clone());
                    destination.merged = Some(super::batch::MergedPomodoro {
                        before,
                        after,
                        added,
                    });
                }
            }
            if day_relative.is_none() {
                day_relative = Some(capture_pomodoros::relative_day_file(
                    &day_path,
                    &request.bob_dir,
                ));
            }
        }
        let _ = current_destination;
    }

    // Re-resolve every moved root by stable block-ID identity after the
    // item's links: dependency-child removals, schedule-log edits, and
    // later same-note writes shift destination rows, so carrying the
    // pre-link landing line reports stale lines (e.g. c on 15 vs 14).
    // Preserve sigil, source coordinates, UTF-8 ranges, previous status
    // and move history; never derive identity from title text.
    if let Some(execution) = move_execution.as_mut() {
        if let Ok(final_contents) = planner.read_existing(&new_target) {
            let final_scan = note_tasks::scan(&final_contents, &settings);
            for task in execution.moved_tasks.iter_mut() {
                if let BlockIdLookup::Found(live) =
                    final_scan.by_block_id(&task.block_id)
                {
                    task.line = live.line_index + 1;
                    task.status_symbol = live.status_symbol;
                    task.status_name = task_complete::status_name_for(
                        &request.bob_dir,
                        live.status_symbol,
                    );
                    // Text follows the live row; identity stays block ID.
                    if let Some(line_text) =
                        final_contents.lines().nth(live.line_index)
                    {
                        // Keep the original title for reporting stability
                        // unless the row genuinely changed (e.g. status or
                        // type edits during linking).
                        let _ = line_text;
                    }
                }
            }
        }
        // Refresh followed live-link entries after day insertions and
        // destination-name merges so entry lines/names agree with the
        // final staged day. Selected offers, task links and block tracker
        // rows stay consistent via the same staged state.
        let day_path = pomodoro::day_file_for(&request.bob_dir);
        if let Ok(Some(final_day)) = planner.current_contents(&day_path) {
            let dest_route_refresh = basename
                .strip_suffix(".md")
                .unwrap_or(&basename)
                .to_string();
            for task in execution.moved_tasks.iter_mut() {
                if task.planned.is_some() {
                    task.planned = followed_live_link(
                        &final_day,
                        &dest_route_refresh,
                        &task.block_id,
                    );
                    task.detail = super::move_consequence::describe(
                        &super::move_consequence::Outcome {
                            sigil: task.sigil,
                            previous_status: task.previous_status_symbol,
                            status: task.status_symbol,
                            existing_today_link: task
                                .planned
                                .as_ref()
                                .map(|link| link.entry_name.clone()),
                            added_today_link: None,
                            own_scheduled: None,
                            project_starts: None,
                            guard: super::move_consequence::LinkGuard::Link,
                            id_added: None,
                        },
                    );
                }
            }
        }
        // Keep moved-offer rows consistent with the final staged rows.
        for offer in frozen_offers.iter_mut().filter(|o| o.moved_from.is_some())
        {
            if let Some(block_id) = offer.final_block_id.clone()
                && let Some(moved) = execution
                    .moved_tasks
                    .iter()
                    .find(|t| t.block_id == block_id)
            {
                offer.final_status = moved.status_symbol;
                offer.final_line = moved.line;
            }
        }
    }

    let prj_text = parsed.body.trim().to_string();
    Ok(PlannedCaptureItem {
        result: CaptureItemResult {
            note_pull: None,
            task_theme,
            type_relocation: None,
            ok: true,
            dry_run: request.dry_run,
            routed: true,
            route_label: basename.clone(),
            route: Some(route.clone()),
            relative_target: basename.clone(),
            target: new_target.display().to_string(),
            text: parsed.body,
            task_line: rendered.task_line.clone(),
            kind: "project_note",
            fanout: None,
            created,
            scheduled,
            priority: priority.as_ref().map(|resolved| resolved.value.clone()),
            priority_label: priority
                .as_ref()
                .map(|resolved| resolved.label.clone()),
            placement: Placement::Created,
            sub_bullets: Vec::new(),
            notes: Vec::new(),
            drawers: Vec::new(),
            clip: None,
            schedule_log,
            block_id: Some("prj".to_string()),
            day_file,
            block_link: None,
            pomodoro_link_placement,
            parent_line: None,
            parent_text: None,
            drawer: None,
            parent_status_symbol: None,
            parent_status_name: None,
            toggle_direction: None,
            previous_task_line: None,
            status_symbol: None,
            status_name: None,
            previous_status_symbol: None,
            previous_status_name: None,
            pomodoro_name: canonical_pomodoro_name,
            creates_pomodoro,
            pomodoro_already_linked: None,
            removed_pomodoro_links: None,
            removed_scheduled: None,
            pomodoro_selector_unused: None,
            toggle_behavior: None,
            status_changed: None,
            pomodoro_link_action: None,
            pomodoro_link_source: None,
            pomodoro_link_destination: first_link_destination,
            project_note: Some(ProjectNoteSummary {
                basename: rendered.basename.clone(),
                parent_route: route.clone(),
                parent_link: format!("[[{route}]]"),
                tasks: rendered.task_count,
                sections: rendered.sections.clone(),
                task_links,
                moved_tasks: move_execution
                    .as_ref()
                    .map(|execution| execution.moved_tasks.clone())
                    .unwrap_or_default(),
                move_summary: move_execution
                    .as_ref()
                    .map(|execution| execution.summary.clone()),
                repaired_files: move_execution
                    .as_ref()
                    .map(|execution| execution.repaired_files.clone()),
                move_hint,
                form: (form == ProjectNoteForm::Offer).then_some("offer"),
                successors: (form == ProjectNoteForm::Offer).then(|| {
                    let mut scratch_index = 0;
                    // Final day after all actual links for moved selected
                    // successor link line lookups.
                    let successor_day_path =
                        pomodoro::day_file_for(&request.bob_dir);
                    let successor_final_day = planner
                        .peek_text(&successor_day_path)
                        .unwrap_or_default();
                    frozen_offers
                        .iter()
                        .map(|offer| {
                            // Moved selected offers link via the existing-task
                            // gesture (saved in moved_selected_successor);
                            // new selected use actual placements, unselected
                            // (new + moved) use recomputed scratch previews.
                            if offer.moved_from.is_some() && offer.selected {
                                let block_link =
                                    offer.final_block_id.as_ref().map(
                                        |block_id| {
                                            format!(
                                                "[[{stem}#^{block_id}]]"
                                            )
                                        },
                                    );
                                let link = offer
                                    .final_block_id
                                    .as_ref()
                                    .and_then(|block_id| {
                                        moved_selected_successor
                                            .get(block_id)
                                    })
                                    .zip(block_link.as_ref())
                                    .map(
                                        |(
                                            (entry_name,
                                            entry_line,
                                            entry_created,
                                            next_up),
                                            block_link,
                                        )| {
                                            TaskCompleteSuccessorLinkJson {
                                                day_file: day_relative
                                                    .clone()
                                                    .unwrap_or_default(),
                                                entry_name: entry_name
                                                    .clone(),
                                                entry_line: *entry_line,
                                                entry_created: *entry_created,
                                                next_up: *next_up,
                                                line: block_link_line(
                                                    &successor_final_day,
                                                    block_link,
                                                ),
                                                block_link: block_link
                                                    .clone(),
                                                block_id_created: false,
                                            }
                                        },
                                    );
                                let (entry_role, fallback) = link
                                    .as_ref()
                                    .map(|_| ("slot", false))
                                    .unwrap_or(("implicit", true));
                                let destination_name = link
                                    .as_ref()
                                    .map(|link| link.entry_name.clone())
                                    .unwrap_or_default();
                                let _ = (entry_role, fallback);
                                return TaskCompleteUnblockedJson {
                                    note_path: basename.clone(),
                                    block_id: offer
                                        .final_block_id
                                        .clone()
                                        .unwrap_or_default(),
                                    line: offer.final_line,
                                    text: offer.text.clone(),
                                    previous_status_symbol: offer.pre_status,
                                    previous_status_name: status_name_for(
                                        &request.bob_dir,
                                        offer.pre_status,
                                    ),
                                    status_symbol: offer.final_status,
                                    status_name: status_name_for(
                                        &request.bob_dir,
                                        offer.final_status,
                                    ),
                                    inbox: false,
                                    unblocked_by: vec![
                                        TaskCompleteUnblockedCauseJson {
                                            note_path: basename.clone(),
                                            block_id: "prj".to_string(),
                                            text: prj_text.clone(),
                                        },
                                    ],
                                    link,
                                    not_linked: None,
                                    trigger: crate::native::task_complete::PROJECT_CREATED_TRIGGER,
                                    hide_removed: false,
                                    offer: Some(TaskCompleteOfferJson {
                                        key: offer.key.clone(),
                                        n: offer.n,
                                        group: "project",
                                        selected: offer.selected,
                                        block_id_needed: offer.block_id_needed,
                                        project: Some(basename.clone()),
                                        after: None,
                                        moved_from: offer.moved_from.clone(),
                                        merged_from: None,
                                        note: None,
                                        destination:
                                            TaskCompleteOfferDestinationJson {
                                                day_file: day_relative
                                                    .clone()
                                                    .unwrap_or_default(),
                                                entry_name: destination_name,
                                                entry_role,
                                                fallback,
                                            },
                                        order_key: offer.order_key.clone(),
                                        settled_ordinal: offer.settled_ordinal,
                                    }),
                                    relation: None,
                                };
                            }
                            let placement = if offer.selected {
                                actual_placements.iter().find(|placed| {
                                    Some(placed.task.block_id.as_str())
                                        == offer.final_block_id.as_deref()
                                })
                            } else {
                                let placed =
                                    scratch_placements.get(scratch_index);
                                scratch_index += 1;
                                placed
                            };
                            let (entry_role, fallback) = placement
                                .map(|placed| {
                                    offer_entry_role(
                                        pomodoro_name.as_deref(),
                                        placed.used_typed,
                                        placed.insertion.creates_pomodoro,
                                    )
                                })
                                .unwrap_or(("implicit", true));
                            let destination_name = placement
                                .and_then(|placed| {
                                    placed.insertion.destination.name.clone()
                                })
                                .unwrap_or_default();
                            let block_link = offer.final_block_id.as_ref().map(
                                |block_id| {
                                    format!("[[{stem}#^{block_id}]]")
                                },
                            );
                            let link = if offer.selected {
                                placement.zip(block_link.as_ref()).map(
                                    |(placed, block_link)| {
                                        TaskCompleteSuccessorLinkJson {
                                            day_file: day_relative
                                                .clone()
                                                .unwrap_or_default(),
                                            entry_name: destination_name
                                                .clone(),
                                            entry_line: placed
                                                .insertion
                                                .destination
                                                .line,
                                            entry_created: placed
                                                .insertion
                                                .creates_pomodoro,
                                            next_up: placed
                                                .insertion
                                                .destination
                                                .role
                                                == Some("next_up"),
                                            line: block_link_line(
                                                &placed.insertion.updated,
                                                block_link,
                                            ),
                                            block_link: block_link.clone(),
                                            block_id_created: offer
                                                .block_id_needed,
                                        }
                                    },
                                )
                            } else {
                                None
                            };
                            TaskCompleteUnblockedJson {
                                note_path: basename.clone(),
                                block_id: offer
                                    .final_block_id
                                    .clone()
                                    .unwrap_or_default(),
                                line: offer.final_line,
                                text: offer.text.clone(),
                                previous_status_symbol: offer.pre_status,
                                previous_status_name: status_name_for(
                                    &request.bob_dir,
                                    offer.pre_status,
                                ),
                                status_symbol: offer.final_status,
                                status_name: status_name_for(
                                    &request.bob_dir,
                                    offer.final_status,
                                ),
                                inbox: false,
                                unblocked_by: vec![
                                    TaskCompleteUnblockedCauseJson {
                                        note_path: basename.clone(),
                                        block_id: "prj".to_string(),
                                        text: prj_text.clone(),
                                    },
                                ],
                                link,
                                not_linked: None,
                                trigger: crate::native::task_complete::PROJECT_CREATED_TRIGGER,
                                hide_removed: false,
                                offer: Some(TaskCompleteOfferJson {
                                    key: offer.key.clone(),
                                    n: offer.n,
                                    group: "project",
                                    selected: offer.selected,
                                    block_id_needed: offer.block_id_needed,
                                    project: Some(basename.clone()),
                                    after: None,
                                    moved_from: offer.moved_from.clone(),
                                    merged_from: None,
                                    note: None,
                                    destination:
                                        TaskCompleteOfferDestinationJson {
                                            day_file: day_relative
                                                .clone()
                                                .unwrap_or_default(),
                                            entry_name: destination_name,
                                            entry_role,
                                            fallback,
                                        },
                                    order_key: offer.order_key.clone(),
                                    settled_ordinal: offer.settled_ordinal,
                                }),
                                relation: None,
                            }
                        })
                        .collect()
                }),
                project_folds,
            }),
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_split: None,
            pomodoro_merge: None,
            pomodoro_close: None,
            pomodoro_reset: None,
            dependency_update: None,
            consumed_links: Vec::new(),
            task_complete: None,
            toggle_task_description: None,
            r#ref: None,
            link_guard: project_link_guard,
            dropped_dependencies: extra_dropped,
            blocks_line_updates: extra_blocks_updates,
            minted_dependent_block_id: None,
            dependency_updates: Vec::new(),
            minted_blocker_block_id: None,
            relation_updates: Vec::new(),
            relation_cancels: Vec::new(),
            relation_completes: Vec::new(),
            relation_merge_output: Default::default(),
            transfer_move_output: Default::default(),
            linked_from_line_updates: Vec::new(),
            minted_relation_block_ids: Vec::new(),
            mirror_skipped: Vec::new(),
            list: None,
            relation_label_hides: Vec::new(),
            relation_successors: Vec::new(),
            relation_held: Vec::new(),
            relation_check: None,
            relation_groups: Vec::new(),
        },
        clip_plan: None,
        pomodoro_refs: {
            let mut refs: Vec<super::PomodoroBlockRef> = project_merges
                .iter()
                .map(|merge| {
                    super::PomodoroBlockRef::at(
                        super::PomodoroBlockRole::Changed,
                        merge.dest_index,
                        merge.dest_index,
                    )
                })
                .collect();
            refs.extend(extra_pomodoro_refs);
            refs
        },
        task_block_refs: extra_task_block_refs,
        project_move_landings: move_execution.as_ref().map_or_else(
            Vec::new,
            |execution| {
                let dest_relative = basename.clone();
                let dest_route = basename
                    .strip_suffix(".md")
                    .unwrap_or(&basename)
                    .to_string();
                execution
                    .moved_tasks
                    .iter()
                    .map(|task| super::ProjectMoveLanding {
                        source: source_path.clone(),
                        dest: new_target.clone(),
                        dest_relative: dest_relative.clone(),
                        dest_route: dest_route.clone(),
                        old_block_id: Some(task.block_id.clone()),
                        new_block_id: Some(task.block_id.clone()),
                        old_line: task.source_line,
                        new_line: task.line,
                    })
                    .collect()
            },
        ),
    })
}

struct ResolvedProjectMove {
    source_index: usize,
    sigil: char,
    block_id: String,
    draft_range: (usize, usize),
    root: MovableRoot,
    own_scheduled: Option<String>,
}

fn project_note_move_hint(
    planner: &CaptureBatchPlanner,
    source_path: &Path,
    scheduled: Option<&str>,
    today: NaiveDate,
    bob_dir: &Path,
) -> Option<ProjectNoteMoveHint> {
    let contents = planner.peek_text(source_path)?;
    let future_project_date = scheduled.and_then(|value| {
        NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .ok()
            .filter(|date| *date > today)
    });
    let lines: Vec<&str> = contents.lines().collect();
    let relative_source =
        source_path.strip_prefix(bob_dir).unwrap_or(source_path);
    let roots = movable_roots(relative_source, &contents);
    let open_count = roots
        .into_iter()
        .filter(|root| root.disabled_reason.is_none())
        .filter(|root| {
            !lines.get(root.line.saturating_sub(1)).is_some_and(|line| {
                line.contains(capture_project_note::PLACEHOLDER_TASK_BODY)
            })
        })
        .filter(|root| future_project_date.is_none() || root.status == '?')
        .count();
    (open_count > 0).then(|| ProjectNoteMoveHint {
        parent: display_project_path(relative_source),
        open_count,
    })
}

struct ProjectMoveExecution {
    moved_tasks: Vec<ProjectNoteMovedTask>,
    summary: ProjectNoteMoveSummary,
    repaired_files: Vec<ProjectNoteRepairedFile>,
}

#[allow(clippy::too_many_arguments)]
fn resolve_project_note_moves(
    planner: &CaptureBatchPlanner,
    source_path: &Path,
    route: &str,
    destination_basename: &str,
    sub_bullets: &[AuthoredSubBullet],
    scheduled: Option<&str>,
    today: NaiveDate,
    bob_dir: &Path,
) -> Result<Vec<ResolvedProjectMove>, CaptureError> {
    let moves: Vec<(usize, &crate::native::capture_language::MoveBullet)> =
        sub_bullets
            .iter()
            .enumerate()
            .filter_map(|(index, bullet)| {
                bullet.move_task.as_ref().map(|movement| (index, movement))
            })
            .collect();
    if moves.is_empty() {
        return Ok(Vec::new());
    }
    let Some(contents) = planner.peek_text(source_path) else {
        return Err(CaptureError::io(format!(
            "project-note parent disappeared while planning: {}",
            source_path.display()
        )));
    };
    let parent_label = format!("{route}.md");
    let destination_label = destination_basename
        .strip_suffix(".md")
        .unwrap_or(destination_basename)
        .to_string();
    let future_project_date = scheduled.and_then(|value| {
        NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .ok()
            .filter(|date| *date > today)
    });
    let mut resolved = Vec::with_capacity(moves.len());
    for (source_index, movement) in moves {
        let Some(block_id) = movement.block_id.as_deref() else {
            return Err(CaptureError::io(
                "project-note move invariant failed: block ID is missing",
            ));
        };
        if let Some(previous) = planner.moved_origin(source_path, block_id) {
            let destination = previous
                .destination
                .strip_prefix(bob_dir)
                .unwrap_or(&previous.destination);
            return Err(CaptureError::usage_code(
                format!(
                    "`^{block_id}` already moved to {} in item {}",
                    display_project_path(destination),
                    previous.item_number
                ),
                "move_task_already_moved",
            ));
        }
        let root = resolve_movable(&contents, block_id).map_err(|reason| {
            project_move_ineligible_error(
                reason,
                block_id,
                &parent_label,
                bob_dir,
            )
        })?;
        let own_scheduled = contents
            .lines()
            .nth(root.line.saturating_sub(1))
            .and_then(scheduled_field);
        if future_project_date.is_some() && root.status != '?' {
            let date = scheduled.unwrap_or_default();
            let status_name =
                task_complete::status_name_for(bob_dir, root.status);
            return Err(CaptureError::usage_code(
                format!(
                    "`s:{}` starts {} on {date}, so it cannot take {status_name} task `^{block_id}` yet; drop `s:{}` or move the task after it starts",
                    scheduled_offset_from_date(today, date),
                    destination_label,
                    scheduled_offset_from_date(today, date),
                ),
                "move_task_blocked_birth",
            ));
        }
        resolved.push(ResolvedProjectMove {
            source_index,
            sigil: movement.sigil,
            block_id: block_id.to_string(),
            draft_range: movement.range,
            root,
            own_scheduled,
        });
    }
    Ok(resolved)
}

fn project_move_ineligible_error(
    reason: Ineligible,
    block_id: &str,
    parent_label: &str,
    bob_dir: &Path,
) -> CaptureError {
    let task = format!("`^{block_id}`");
    let (message, code) = match reason {
        Ineligible::Missing => (
            format!(
                "{parent_label} has no open task {task}; type `^` after `- ` to pick one of its open tasks"
            ),
            "move_task_missing",
        ),
        Ineligible::Closed { status } => {
            let name = task_complete::status_name_for(bob_dir, status);
            (
                format!("{task} in {parent_label} is {}; only open tasks move into a new project", name.to_lowercase()),
                "move_task_closed",
            )
        }
        Ineligible::Nested {
            parent_id,
            parent_title,
        } => {
            let parent = parent_id
                .map(|id| format!("`^{id}`"))
                .unwrap_or_else(|| format!("`{parent_title}`"));
            (
                format!("{task} is nested under {parent} in {parent_label}; move {parent} to bring the whole block"),
                "move_task_nested",
            )
        }
        Ineligible::Ambiguous { count } => (
            format!(
                "{parent_label} has {count} tasks ending {task}; give one a new ID before moving it"
            ),
            "move_task_ambiguous",
        ),
        Ineligible::Project => (
            format!("{task} is a project's own task and cannot move"),
            "",
        ),
        Ineligible::UnsupportedCheckbox { status } => (
            format!(
                "refusing task {task} in {parent_label} with checkbox [{status}]"
            ),
            "",
        ),
    };
    if code.is_empty() {
        CaptureError::usage(message)
    } else {
        CaptureError::usage_code(message, code)
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_project_note_moves(
    request: &CaptureRequest,
    planner: &mut CaptureBatchPlanner,
    source_path: &Path,
    new_target: &Path,
    route: &str,
    basename: &str,
    rendered: &capture_project_note::RenderedProjectNote,
    moves: &[ResolvedProjectMove],
    settings: &note_tasks::NoteTaskSettings,
    today: NaiveDate,
    item_number: usize,
) -> Result<ProjectMoveExecution, CaptureError> {
    let slot_by_source: HashMap<usize, usize> = rendered
        .move_slots
        .iter()
        .map(|slot| (slot.source_index, slot.slot))
        .collect();
    let selectors = moves
        .iter()
        .map(|movement| MoveSelector::Block(movement.block_id.clone()))
        .collect::<Vec<_>>();
    let slots = moves
        .iter()
        .map(|movement| {
            slot_by_source.get(&movement.source_index).copied().ok_or_else(
                || {
                    CaptureError::io(
                        "project-note move invariant failed: task slot is missing",
                    )
                },
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    planner.stage(new_target, rendered.contents.clone())?;
    let destination_route = basename.strip_suffix(".md").unwrap_or(basename);
    let move_request = MoveRequest {
        source: route.to_string(),
        dest: destination_route.to_string(),
        selectors,
        allow_closed: false,
        placement: MovePlacement::CaptureSlots(slots),
        id_collision: IdCollisionPolicy::Refuse,
    };
    let move_plan = match apply_batch_move(
        planner,
        &request.bob_dir,
        settings,
        today,
        &move_request,
    ) {
        Ok(plan) => plan,
        Err(BatchMoveError::Plan(error)) => {
            return Err(project_move_engine_error(
                error,
                &rendered.contents,
                settings,
                route,
            ));
        }
        Err(BatchMoveError::Capture(error)) => return Err(error),
    };

    let destination_contents =
        planner.peek_text(new_target).ok_or_else(|| {
            CaptureError::io(format!(
                "capture move lost destination note {}",
                new_target.display()
            ))
        })?;
    let destination_scan = note_tasks::scan(&destination_contents, settings);
    let day_file = pomodoro::day_file_for(&request.bob_dir);
    // Staged-aware: a never-loaded disk day must agree with the same day
    // already staged by an earlier item.
    let day_contents = planner.current_contents(&day_file).ok().flatten();
    let mut moved_tasks = Vec::with_capacity(moves.len());
    for authored in moves {
        let movement = move_plan
            .moves
            .iter()
            .find(|record| {
                record.old_block_id.as_deref() == Some(&authored.block_id)
            })
            .ok_or_else(|| {
                CaptureError::io(format!(
                    "capture move lost result for ^{}",
                    authored.block_id
                ))
            })?;
        let landed = match destination_scan.by_block_id(&authored.block_id) {
            BlockIdLookup::Found(task) => task,
            _ => {
                return Err(CaptureError::io(format!(
                    "capture move lost block ^{} in {}",
                    authored.block_id, basename
                )));
            }
        };
        let landing_line = destination_contents
            .lines()
            .nth(landed.line_index)
            .unwrap_or_default();
        let fresh = crate::native::freshness::placement::read_freshness(
            landing_line,
            today,
        )
        .fresh
        .map(|date| date.to_string());
        let planned = day_contents.as_deref().and_then(|day| {
            followed_live_link(day, destination_route, &authored.block_id)
        });
        let detail = super::move_consequence::describe(
            &super::move_consequence::Outcome {
                sigil: authored.sigil,
                previous_status: authored.root.status,
                status: landed.status_symbol,
                existing_today_link: planned
                    .as_ref()
                    .map(|link| link.entry_name.clone()),
                added_today_link: None,
                own_scheduled: authored.own_scheduled.clone(),
                project_starts: None,
                guard: super::move_consequence::LinkGuard::Link,
                id_added: None,
            },
        );
        let source = display_project_path(&movement.source_path);
        moved_tasks.push(ProjectNoteMovedTask {
            sigil: authored.sigil,
            draft_range: authored.draft_range,
            item_line: draft_line_number(
                &request.raw_text,
                authored.draft_range.0,
            ),
            source,
            source_line: movement.source_line,
            line: movement.dest_line,
            block_id: authored.block_id.clone(),
            text: movement.title.clone(),
            previous_status_symbol: authored.root.status,
            previous_status_name: task_complete::status_name_for(
                &request.bob_dir,
                authored.root.status,
            ),
            status_symbol: landed.status_symbol,
            status_name: task_complete::status_name_for(
                &request.bob_dir,
                landed.status_symbol,
            ),
            fresh,
            planned,
            link: None,
            not_linked: None,
            detail,
        });
        planner.record_moved_origin(
            source_path,
            &authored.block_id,
            new_target,
            item_number,
        );
    }

    let repaired_files = move_plan
        .repaired_files
        .iter()
        .map(|file| ProjectNoteRepairedFile {
            path: display_project_path(&file.path),
            link_count: file.link_count,
            dependency_token_count: file.dependency_token_count,
        })
        .collect::<Vec<_>>();
    let repaired_links =
        repaired_files.iter().map(|file| file.link_count).sum();
    Ok(ProjectMoveExecution {
        moved_tasks,
        summary: ProjectNoteMoveSummary {
            parent: display_project_path(
                source_path
                    .strip_prefix(&request.bob_dir)
                    .unwrap_or(source_path),
            ),
            moved: move_plan.moves.len(),
            repaired_links,
            repaired_notes: repaired_files.len(),
            needs_reconcile: move_plan.needs_reconcile,
        },
        repaired_files,
    })
}

fn scheduled_field(line: &str) -> Option<String> {
    task_fields::inline_fields(line, "scheduled")
        .into_iter()
        .find_map(|field| {
            task_fields::parse_strict_calendar_date(&field.value)
                .map(|date| date.format("%b %-d").to_string())
        })
}

fn display_move_date(value: &str) -> String {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map(|date| date.format("%b %-d").to_string())
        .unwrap_or_else(|_| value.to_string())
}

/// Link-guard detail is written for reports (`not linked (...) · promoted…`).
/// Move copy keeps the blocking reason while removing the report wrapper and
/// reducing task wikilinks to their short block IDs.
fn move_guard_reason(detail: &str) -> String {
    let mut reason = detail.trim();
    if let Some(rest) = reason.strip_prefix("not linked (") {
        reason = rest
            .split_once(") ·")
            .map(|(reason, _)| reason)
            .or_else(|| rest.strip_suffix(')'))
            .unwrap_or(rest);
    }
    let mut output = String::with_capacity(reason.len());
    let mut rest = reason;
    while let Some(start) = rest.find("[[") {
        output.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];
        let Some(end) = after_open.find("]]") else {
            output.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let target = &after_open[..end];
        let short = target
            .rsplit_once("#^")
            .map(|(_, id)| format!("^{id}"))
            .unwrap_or_else(|| target.to_string());
        output.push_str(&short);
        rest = &after_open[end + 2..];
    }
    output.push_str(rest);
    output.trim().to_string()
}

fn project_move_engine_error(
    error: PlanError,
    destination_contents: &str,
    settings: &note_tasks::NoteTaskSettings,
    route: &str,
) -> CaptureError {
    let PlanError::BlockIdCollision {
        candidate,
        block_id,
        ..
    } = &error
    else {
        return CaptureError::usage(error.to_string());
    };
    let collision_line = note_tasks::scan(destination_contents, settings)
        .by_block_id(block_id)
        .found_line();
    let moved_root = candidate.block_id.as_deref().unwrap_or("?");
    let message = if block_id != moved_root {
        match collision_line {
            Some(line) => format!(
                "`^{block_id}` under moved `^{moved_root}` collides with task ID `{block_id}` on line {line}; rename the new task"
            ),
            None => format!(
                "`^{block_id}` under moved `^{moved_root}` collides with another task ID in {route}.md; rename the new task"
            ),
        }
    } else {
        match collision_line {
            Some(line) => format!(
                "task ID `{block_id}` is moved from {route}.md on line {}; rename the new task on line {line}",
                candidate.line
            ),
            None => format!(
                "task ID `{block_id}` is moved from {route}.md; rename the new task"
            ),
        }
    };
    CaptureError::usage_code(message, "move_task_id_collision")
}

trait FoundLine {
    fn found_line(self) -> Option<usize>;
}

impl<'a> FoundLine for BlockIdLookup<'a> {
    fn found_line(self) -> Option<usize> {
        match self {
            BlockIdLookup::Found(task) => Some(task.line_index + 1),
            BlockIdLookup::Missing
            | BlockIdLookup::Duplicate(_)
            | BlockIdLookup::NotATask { .. } => None,
        }
    }
}

fn followed_live_link(
    day_contents: &str,
    destination_route: &str,
    block_id: &str,
) -> Option<ProjectNotePlannedLink> {
    // Exact staged-aware dedicated-link identity via the shared ledger:
    // sole-content [[route#^id]] bullets under open Pomodoros only. Prose
    // mentions, historical/closed entries, fenced/struck/deeper-nested rows,
    // and prefix IDs never count.
    let ledger = capture_active_tasks::ledger_from_contents(day_contents);
    ledger
        .owners
        .get(&(destination_route.to_string(), block_id.to_string()))
        .map(|pomodoro| ProjectNotePlannedLink {
            entry_name: pomodoro.name.clone().unwrap_or_default(),
            entry_line: pomodoro.line,
        })
}

/// Whether the staged parent project blocks a new child project via
/// inheritance: a future schedule on the parent ^prj or an existing
/// #prj_is_blocked proxy (own or inherited) means the child inherits.
/// Uses only the already-loaded staged parent text, so no-move drafts gain
/// no new vault-wide scans. Deeper ancestors surface through the parent's
/// own tagged ^prj; staged parents work because current_contents returns
/// the staged overlay.
fn parent_project_blocks_child(
    planner: &mut CaptureBatchPlanner,
    source_path: &Path,
    today: NaiveDate,
    settings: &note_tasks::NoteTaskSettings,
) -> bool {
    let Ok(Some(contents)) = planner.current_contents(source_path) else {
        return false;
    };
    let scan = note_tasks::scan(&contents, settings);
    // Parent must be a recognised project with an open ^prj to inherit.
    let tasks =
        crate::native::task_status_hooks::parse_tasks(&contents, settings);
    let relative = source_path
        .file_name()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("parent.md"));
    // Rebuild a minimal FileScan for ProjectContext: relative path is only
    // used for identity, contents/tasks drive recognition.
    let file_scan = crate::native::task_status_hooks::FileScan {
        path: source_path.to_path_buf(),
        relative_path: relative,
        contents: contents.clone(),
        tasks: tasks.clone(),
    };
    let context = crate::native::project_dependencies::ProjectContext::for_note(
        &file_scan,
    );
    if !context.recognised {
        return false;
    }
    // Future schedule on the parent ^prj blocks children.
    if context.scheduled.is_some_and(|date| date > today) {
        return true;
    }
    // An existing proxy tag means the parent is itself blocked (own or
    // inherited) and reconcile has recorded it; the child inherits.
    let lines: Vec<&str> = contents.lines().collect();
    for task in scan.tasks() {
        let Some(line) = lines.get(task.line_index) else {
            continue;
        };
        if crate::native::capture_dependency_tasks::has_prj_is_blocked_tag(line)
        {
            return true;
        }
    }
    false
}

/// Tag the freshly rendered child note with the inherited Blocked proxy:
/// open ^prj becomes [?] plus tag, open residents become [?] plus tag.
/// The parent keeps its own reasons; no dates/prerequisites are copied.
fn apply_inherited_block_to_rendered(
    rendered: &mut capture_project_note::RenderedProjectNote,
    settings: &note_tasks::NoteTaskSettings,
) {
    // Patch the ^prj lifecycle line.
    let patched_prj = patch_task_line_to_blocked(&rendered.task_line, settings);
    rendered.task_line = patched_prj.clone();
    // Patch the full contents: first occurrence of the ^prj line plus every
    // open resident task line. Closed/placeholder rows stay untouched.
    let mut lines: Vec<String> =
        rendered.contents.lines().map(str::to_string).collect();
    let mut prj_patched = false;
    for line in lines.iter_mut() {
        // Only first-level task lines can be project residents; nested,
        // fenced and section lines never carry the proxy from rendering.
        let trimmed = line.trim_start();
        if !(trimmed.starts_with("- ") || trimmed.starts_with("* ")) {
            continue;
        }
        // Parse status symbol: "- [X]" shape.
        let bytes = trimmed.as_bytes();
        if bytes.len() < 6 || bytes[0] != b'-' && bytes[0] != b'*' {
            continue;
        }
        // Find "[X]" after the marker.
        let Some(open) = trimmed.find('[') else {
            continue;
        };
        let Some(close) = trimmed.find(']') else {
            continue;
        };
        if close != open + 2 {
            continue;
        }
        let symbol = trimmed.as_bytes()[open + 1] as char;
        // Skip closed, placeholder and already-blocked rows; keep the
        // first ^prj patch aligned with rendered.task_line.
        if symbol == 'x' || symbol == 'X' || symbol == '-' {
            continue;
        }
        if line.contains(capture_project_note::PLACEHOLDER_TASK_BODY) {
            continue;
        }
        let Some(status_type) = settings.status_types.get(&symbol) else {
            continue;
        };
        if !status_type.is_open() {
            continue;
        }
        let patched = patch_task_line_to_blocked(line, settings);
        // Only patch lines that actually change (open tasks); this keeps
        // headings, logs and drawers byte-identical.
        if patched != *line {
            // For the ^prj line, reuse the already-patched task_line once.
            if !prj_patched && line.contains("^prj") {
                *line = patched_prj.clone();
                prj_patched = true;
            } else {
                *line = patched;
            }
        } else if !prj_patched && line.contains("^prj") {
            *line = patched_prj.clone();
            prj_patched = true;
        }
    }
    // Ensure the ^prj line in contents matches rendered.task_line even
    // when the scan above missed it (e.g. unusual spacing).
    if !prj_patched {
        for line in lines.iter_mut() {
            if line.contains("^prj") {
                *line = patched_prj.clone();
                break;
            }
        }
    }
    rendered.contents = lines.join("\n");
    if !rendered.contents.ends_with('\n') {
        rendered.contents.push('\n');
    }
}

fn patch_task_line_to_blocked(
    line: &str,
    settings: &note_tasks::NoteTaskSettings,
) -> String {
    use crate::native::projects::edits::normalize_prj_is_blocked_tag;
    // Change "[X]" to "[?]" for open statuses; leave closed/unknown alone.
    let mut patched = line.to_string();
    if let Some(open) = line.find('[')
        && let Some(close) = line.find(']')
        && close == open + 2
    {
        let symbol = line.as_bytes()[open + 1] as char;
        if let Some(status_type) = settings.status_types.get(&symbol)
            && status_type.is_open()
        {
            patched.replace_range(open + 1..open + 2, "?");
        }
    }
    normalize_prj_is_blocked_tag(&patched, true)
}

fn draft_line_number(text: &str, offset: usize) -> usize {
    text.get(..offset.min(text.len()))
        .map(|prefix| prefix.bytes().filter(|byte| *byte == b'\n').count() + 1)
        .unwrap_or(1)
}

fn display_project_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn scheduled_offset_from_date(today: NaiveDate, scheduled: &str) -> String {
    let Some(scheduled) = NaiveDate::parse_from_str(scheduled, "%Y-%m-%d").ok()
    else {
        return "?".to_string();
    };
    (scheduled - today).num_days().to_string()
}

struct FrozenOffer {
    key: String,
    n: usize,
    order_key: crate::native::task_complete::OfferOrderKey,
    source_index: Option<usize>,
    pre_status: char,
    text: String,
    selected: bool,
    final_block_id: Option<String>,
    block_id_needed: bool,
    final_status: char,
    final_line: usize,
    moved_from: Option<String>,
    /// Position among the equal-description tasks that stay ID-less after
    /// the picks mint theirs; see `TaskCompleteOfferJson::settled_ordinal`.
    settled_ordinal: Option<usize>,
}

struct PlacedProjectLink {
    task: capture_project_note::RenderedNamedTask,
    insertion: PomodoroBlockLinkInsertion,
    used_typed: bool,
}

fn is_offerable_row(
    row: &capture_project_note::RenderedTask,
    status_types: &BTreeMap<
        char,
        crate::native::task_status_hooks::TaskStatusType,
    >,
    bob_dir: &Path,
) -> bool {
    if row.placeholder || row.link || row.hidden {
        return false;
    }
    if row.block_id.as_deref() == Some("prj") {
        return false;
    }
    let name = status_name_for(bob_dir, row.status);
    let Some(status_type) = status_types.get(&row.status) else {
        return false;
    };
    if !status_type.is_open() || name.eq_ignore_ascii_case("blocked") {
        return false;
    }
    matches!(
        status_type,
        crate::native::task_status_hooks::TaskStatusType::Todo
            | crate::native::task_status_hooks::TaskStatusType::InProgress
            | crate::native::task_status_hooks::TaskStatusType::OnHold
    )
}

fn digest_ordinal_for(
    rows: &[capture_project_note::RenderedTask],
    index: usize,
    global_filter: &str,
) -> usize {
    if rows[index].block_id.is_some() {
        return 1;
    }
    let cleaned =
        note_tasks::clean_description(&rows[index].text, global_filter, None);
    1 + rows[..index]
        .iter()
        .filter(|row| {
            row.block_id.is_none()
                && note_tasks::clean_description(&row.text, global_filter, None)
                    == cleaned
        })
        .count()
}

/// The ordinal of the ID-less row that came from `source_index` among the
/// equal-description rows that still have no ID in `rows`. Rows with an ID
/// (authored or minted by a pick) resolve by that ID instead and have none.
fn settled_ordinal_for(
    rows: &[capture_project_note::RenderedTask],
    source_index: Option<usize>,
    global_filter: &str,
) -> Option<usize> {
    let source_index = source_index?;
    let index = rows
        .iter()
        .position(|row| row.source_index == Some(source_index))?;
    rows[index]
        .block_id
        .is_none()
        .then(|| digest_ordinal_for(rows, index, global_filter))
}

fn offer_entry_role(
    pomodoro_name: Option<&str>,
    used_typed: bool,
    creates: bool,
) -> (&'static str, bool) {
    if creates {
        ("created", false)
    } else if pomodoro_name.is_some() || used_typed {
        ("typed", false)
    } else {
        ("implicit", true)
    }
}

fn block_link_line(contents: &str, block_link: &str) -> usize {
    contents
        .lines()
        .position(|line| line.contains(block_link))
        .map(|index| index + 1)
        .unwrap_or(0)
}

#[allow(clippy::too_many_arguments)]
fn place_project_task_links(
    mut day: String,
    tasks: &[capture_project_note::RenderedNamedTask],
    stem: &str,
    note_route: &str,
    pomodoro_name: Option<&str>,
    pomodoro_name_is_literal: bool,
    planner: &mut CaptureBatchPlanner,
    task_theme_context: &mut CaptureTaskThemeContext,
    warnings: &mut Vec<CaptureWarning>,
    reject_duplicates: bool,
) -> Result<(String, Vec<PlacedProjectLink>), CaptureError> {
    let mut placed = Vec::with_capacity(tasks.len());
    for task in tasks {
        let block_link =
            format!("[[{stem}#^{block_id}]]", block_id = task.block_id);
        if reject_duplicates && day.contains(&block_link) {
            return Err(CaptureError::io(format!(
                "Pomodoro ledger already contains {block_link}"
            )));
        }
        let placement_type = if pomodoro_name.is_none() {
            task_theme_context.placement_for_task(
                planner,
                note_route,
                &task.task_line,
                warnings,
            )
        } else {
            None
        };
        let typed_pair = placement_type
            .as_ref()
            .map(|placement| (placement.as_typed(), placement.placed_type()));
        let (typed, placed_type) = match typed_pair.as_ref() {
            Some((typed, placed_type)) => {
                (Some(typed), Some(placed_type.clone()))
            }
            None => (None, None),
        };
        let mut insertion = insert_pomodoro_block_link(
            &day,
            &block_link,
            pomodoro_name,
            pomodoro_name_is_literal,
            typed,
        )?;
        if let Some(placed_type) = placed_type {
            insertion.destination.placed_type = Some(placed_type);
        }
        day = insertion.updated.clone();
        placed.push(PlacedProjectLink {
            task: task.clone(),
            insertion,
            used_typed: typed.is_some(),
        });
    }
    Ok((day, placed))
}

fn named_task_for_offer(
    row: &capture_project_note::RenderedTask,
    block_id: &str,
) -> capture_project_note::RenderedNamedTask {
    let mut task_line = row.task_line.clone();
    if row.block_id.is_none() {
        task_line.push_str(&format!(" ^{block_id}"));
    }
    capture_project_note::RenderedNamedTask {
        block_id: block_id.to_string(),
        link: true,
        text: row.text.clone(),
        task_line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::capture_language::{
        AuthoredDepth, AuthoredSubBullet, CaptureKind, ParsedCaptureText,
        ProjectNoteForm, ProjectTaskId,
    };
    use crate::native::env::TestEnvGuard;
    use crate::native::task_complete::SuccessorSelection;
    use chrono::NaiveDate;
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::PathBuf;

    fn test_now() -> chrono::NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 9, 20)
            .expect("valid test date")
            .and_hms_opt(14, 31, 7)
            .expect("valid test time")
    }

    fn bullet(body: &str, depth: AuthoredDepth) -> AuthoredSubBullet {
        AuthoredSubBullet {
            body: body.to_string(),
            depth,
            task_id: None,
            move_task: None,
            move_choice: None,
            drawer: None,
        }
    }

    fn named_bullet(
        body: &str,
        depth: AuthoredDepth,
        id: &str,
        link: bool,
    ) -> AuthoredSubBullet {
        AuthoredSubBullet {
            body: body.to_string(),
            depth,
            task_id: Some(ProjectTaskId {
                block_id: id.to_string(),
                link,
            }),
            move_task: None,
            move_choice: None,
            drawer: None,
        }
    }

    fn offer_parsed(
        sub_bullets: Vec<AuthoredSubBullet>,
        pomodoro_name: Option<&str>,
    ) -> ParsedCaptureText {
        ParsedCaptureText {
            body: "Finish the Google exit packet!".to_string(),
            clip: None,
            route: Some("cash".to_string()),
            kind: CaptureKind::ProjectNote {
                block_id: "goog-exit".to_string(),
                pomodoro_name: pomodoro_name.map(str::to_string),
                pomodoro_name_is_literal: false,
                form: ProjectNoteForm::Offer,
            },
            scheduled_offset: None,
            priority_level: None,
            sub_bullets,
            dependencies: Vec::new(),
            blocked_tasks: Vec::new(),
            relations: Vec::new(),
            dependency_target: None,
            type_marker: None,
        }
    }

    fn create_parsed(sub_bullets: Vec<AuthoredSubBullet>) -> ParsedCaptureText {
        let mut parsed = offer_parsed(sub_bullets, None);
        if let CaptureKind::ProjectNote { form, .. } = &mut parsed.kind {
            *form = ProjectNoteForm::Create;
        }
        parsed
    }

    struct OfferHarness {
        _temp: tempfile::TempDir,
        _guard: TestEnvGuard,
        request: CaptureRequest,
        planner: CaptureBatchPlanner,
        task_theme_context: CaptureTaskThemeContext,
        warnings: Vec<CaptureWarning>,
        dependency_ctx: DependencyContext,
        day_file: PathBuf,
    }

    fn harness() -> OfferHarness {
        let temp = tempfile::tempdir().expect("create offer vault");
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).expect("create vault");
        fs::write(
            vault.join("cash.md"),
            "---\ntype: \"[[area]]\"\n---\n# Cash\n",
        )
        .expect("write area");
        let day_file = vault.join("day.md");
        fs::write(
            &day_file,
            "## Pomodoros\n- [ ] (**0900-0930** [t:: 30m]) Work\n",
        )
        .expect("write day file");
        let tasks_settings =
            vault.join(".obsidian/plugins/obsidian-tasks-plugin");
        fs::create_dir_all(&tasks_settings).expect("create Tasks settings");
        fs::write(
            tasks_settings.join("data.json"),
            r##"{
          "globalFilter": "#task",
          "taskFormat": "dataview",
          "statusSettings": {
            "coreStatuses": [
              {"symbol":" ","name":"Ready","nextStatusSymbol":"x","availableAsCommand":true,"type":"TODO"},
              {"symbol":"x","name":"Done","nextStatusSymbol":" ","availableAsCommand":true,"type":"DONE"}
            ],
            "customStatuses": [
              {"symbol":"/","name":"In Progress","nextStatusSymbol":"x","availableAsCommand":true,"type":"IN_PROGRESS"},
              {"symbol":"*","name":"Next","nextStatusSymbol":"x","availableAsCommand":true,"type":"ON_HOLD"},
              {"symbol":"?","name":"Blocked","nextStatusSymbol":" ","availableAsCommand":true,"type":"ON_HOLD"},
              {"symbol":"-","name":"Canceled","nextStatusSymbol":" ","availableAsCommand":true,"type":"CANCELLED"}
            ]
          }
        }"##,
        )
        .expect("write Tasks settings");
        let guard = TestEnvGuard::set(&[
            ("BOB_NOW", Some(std::ffi::OsStr::new("2026-09-20 14:31:07"))),
            ("BOB_DAY_FILE", Some(day_file.as_os_str())),
            ("TZ", Some(std::ffi::OsStr::new("UTC0"))),
        ]);
        let request = CaptureRequest {
            bob_dir: vault,
            dry_run: true,
            forced_clip: None,
            forced_destination_flags: Vec::new(),
            forced_route: None,
            forced_section: None,
            forced_sub_bullet_target: None,
            forced_drawer: None,
            inbox_routes: Vec::new(),
            no_clip: false,
            no_ref: false,
            pull_in: None,
            refuse_override: false,
            require_inbox_route_choice: false,
            require_pull_in_choice: false,
            template_origin: None,
            raw_text: String::new(),
            selection: SuccessorSelection::None,
        };
        let today = test_now().date();
        let dependency_ctx = DependencyContext::new(&request.bob_dir, today);
        OfferHarness {
            planner: CaptureBatchPlanner::default(),
            task_theme_context: CaptureTaskThemeContext::new(
                &request.bob_dir,
                today,
                None,
            ),
            warnings: Vec::new(),
            dependency_ctx,
            request,
            day_file,
            _temp: temp,
            _guard: guard,
        }
    }

    fn plan_offer(
        harness: &mut OfferHarness,
        parsed: ParsedCaptureText,
    ) -> Result<PlannedCaptureItem, CaptureError> {
        plan_project_note_item(
            &harness.request,
            parsed,
            test_now(),
            test_now().date(),
            0,
            0,
            &mut harness.planner,
            &mut harness.task_theme_context,
            &mut harness.warnings,
            &mut harness.dependency_ctx,
        )
    }

    fn example_bullets() -> Vec<AuthoredSubBullet> {
        vec![
            bullet("Draft the memo", AuthoredDepth::First),
            bullet("Collect paperwork", AuthoredDepth::First),
            named_bullet(
                "Call the bank",
                AuthoredDepth::First,
                "call-bank",
                true,
            ),
            bullet("Private reminder #hide", AuthoredDepth::First),
        ]
    }

    #[test]
    fn offer_none_keeps_create_files_and_emits_unselected_rows() {
        let mut harness = harness();
        harness.request.selection = SuccessorSelection::None;
        let planned =
            plan_offer(&mut harness, offer_parsed(example_bullets(), None))
                .expect("plan offer none");
        let note = planned.result.project_note.expect("project note");
        assert_eq!(note.form, Some("offer"));
        let successors = note.successors.expect("successors");
        assert_eq!(successors.len(), 2);
        assert!(successors.iter().all(|row| {
            row.offer.as_ref().is_some_and(|offer| !offer.selected)
                && row.link.is_none()
                && row.trigger
                    == crate::native::task_complete::PROJECT_CREATED_TRIGGER
        }));
        assert_eq!(successors[0].text, "Draft the memo");
        assert_eq!(successors[1].text, "Collect paperwork");
        assert_eq!(successors[0].offer.as_ref().unwrap().n, 1);
        assert_eq!(successors[1].offer.as_ref().unwrap().n, 2);
        assert_eq!(successors[0].unblocked_by[0].block_id, "prj");
        assert!(note
            .task_links
            .iter()
            .all(|link| link.block_id != "draft-the-memo"));
        assert_eq!(note.task_links.len(), 1);
        assert_eq!(note.task_links[0].block_id, "call-bank");
        assert_eq!(note.project_folds[0].hidden, 1);
        assert_eq!(note.project_folds[0].already_planned, 1);
        assert_eq!(note.project_folds[0].sub_projects, 0);
        let staged = harness
            .planner
            .current_contents(
                &harness.request.bob_dir.join("cash_goog_exit.md"),
            )
            .expect("read staged note")
            .expect("note present");
        assert!(staged.contains("- [ ] #task Draft the memo"));
        assert!(!staged.contains("^draft"));
        assert!(!fs::read_to_string(&harness.day_file)
            .expect("read day")
            .contains("cash_goog_exit"));
        let keys: Vec<&str> = successors
            .iter()
            .map(|row| row.offer.as_ref().unwrap().key.as_str())
            .collect();
        assert_ne!(keys[0], keys[1]);
        assert!(keys[0].starts_with("cash_goog_exit.md@"));
    }

    #[test]
    fn offer_all_mints_ids_and_links_like_colon_bullets() {
        let mut harness = harness();
        harness.request.selection = SuccessorSelection::All;
        let planned =
            plan_offer(&mut harness, offer_parsed(example_bullets(), None))
                .expect("plan offer all");
        let note = planned.result.project_note.expect("project note");
        let successors = note.successors.expect("successors");
        assert_eq!(successors.len(), 2);
        assert!(successors.iter().all(|row| {
            row.offer.as_ref().is_some_and(|offer| offer.selected)
                && row.link.is_some()
                && row.status_symbol == '*'
                && row.previous_status_symbol == ' '
                && !row.block_id.is_empty()
        }));
        assert_eq!(note.task_links.len(), 3);
        let staged = harness
            .planner
            .current_contents(
                &harness.request.bob_dir.join("cash_goog_exit.md"),
            )
            .expect("read staged note")
            .expect("note present");
        for row in &successors {
            assert!(staged.contains(&format!("^{}", row.block_id)));
        }
        let day = harness
            .planner
            .current_contents(&harness.day_file)
            .expect("read staged day")
            .expect("day present");
        for row in &successors {
            assert!(
                day.contains(&format!("[[cash_goog_exit#^{}]]", row.block_id))
            );
        }
        assert!(successors.iter().all(|row| {
            row.offer
                .as_ref()
                .is_some_and(|offer| offer.block_id_needed)
        }));
    }

    #[test]
    fn offer_subset_keys_and_mint_collisions() {
        let mut preview_harness = harness();
        preview_harness.request.selection = SuccessorSelection::None;
        let preview = plan_offer(
            &mut preview_harness,
            offer_parsed(example_bullets(), None),
        )
        .expect("preview");
        let key = preview
            .result
            .project_note
            .as_ref()
            .unwrap()
            .successors
            .as_ref()
            .unwrap()[0]
            .offer
            .as_ref()
            .unwrap()
            .key
            .clone();
        let mut harness = harness();
        harness.request.selection =
            SuccessorSelection::Keys(BTreeSet::from([key.clone()]));
        let bullets = vec![
            bullet("Draft the memo", AuthoredDepth::First),
            named_bullet(
                "Keep this id",
                AuthoredDepth::First,
                "draft-the-memo",
                false,
            ),
        ];
        let planned = plan_offer(&mut harness, offer_parsed(bullets, None))
            .expect("subset");
        let successors =
            planned.result.project_note.unwrap().successors.unwrap();
        let selected = successors
            .iter()
            .find(|row| row.offer.as_ref().unwrap().selected)
            .expect("selected row");
        assert_eq!(selected.offer.as_ref().unwrap().key, key);
        assert_ne!(selected.block_id, "draft-the-memo");
        let staged = harness
            .planner
            .current_contents(
                &harness.request.bob_dir.join("cash_goog_exit.md"),
            )
            .expect("read staged note")
            .expect("note present");
        assert!(staged.contains("^draft-the-memo"));
        assert!(staged.contains(&format!("^{}", selected.block_id)));
        assert_eq!(
            successors
                .iter()
                .filter(|row| !row.offer.as_ref().unwrap().selected)
                .count(),
            1
        );
    }

    #[test]
    fn offer_scratch_placement_does_not_commit_probe_ids() {
        let mut harness = harness();
        harness.request.selection = SuccessorSelection::None;
        let planned = plan_offer(
            &mut harness,
            offer_parsed(
                vec![bullet("Draft the memo", AuthoredDepth::First)],
                None,
            ),
        )
        .expect("plan");
        let successors =
            planned.result.project_note.unwrap().successors.unwrap();
        assert_eq!(successors.len(), 1);
        assert!(successors[0].block_id.is_empty());
        assert!(successors[0].offer.as_ref().unwrap().block_id_needed);
        let staged = harness
            .planner
            .current_contents(
                &harness.request.bob_dir.join("cash_goog_exit.md"),
            )
            .expect("read staged note")
            .expect("note present");
        assert!(!staged.contains("^draft"));
        let day = fs::read_to_string(&harness.day_file).expect("read day");
        assert!(!day.contains("cash_goog_exit"));
        let destination = &successors[0].offer.as_ref().unwrap().destination;
        assert!(
            matches!(destination.entry_role, "implicit" | "typed" | "created"),
            "{}",
            destination.entry_role
        );
        assert_eq!(destination.fallback, destination.entry_role == "implicit");
    }

    #[test]
    fn create_omits_offer_fields_and_iterator_skips_them() {
        let mut harness = harness();
        let planned = plan_offer(
            &mut harness,
            create_parsed(vec![bullet("Draft the memo", AuthoredDepth::First)]),
        )
        .expect("plan create");
        let note = planned.result.project_note.as_ref().expect("project note");
        assert_eq!(note.form, None);
        assert_eq!(note.successors, None);
        assert!(note.project_folds.is_empty());
        assert!(all_unblocked(&planned.result).next().is_none());
        let value = serde_json::to_value(note).expect("serialize");
        assert!(value.get("form").is_none(), "{value}");
        assert!(value.get("successors").is_none(), "{value}");
    }

    #[test]
    fn offer_named_without_tasks_is_refused() {
        let mut harness = harness();
        let error = match plan_offer(
            &mut harness,
            offer_parsed(Vec::new(), Some("admin")),
        ) {
            Err(error) => error,
            Ok(_) => panic!("unused name"),
        };
        assert!(
            error
                .message
                .contains("no open task to offer or ` :` task to link"),
            "{}",
            error.message
        );
    }

    #[test]
    fn offer_iterator_and_completion_count_see_project_rows() {
        let mut harness = harness();
        let planned =
            plan_offer(&mut harness, offer_parsed(example_bullets(), None))
                .expect("plan");
        assert_eq!(all_unblocked(&planned.result).count(), 2);
        assert_eq!(all_project_folds(&planned.result).count(), 1);
        assert_eq!(unblocked_vecs_mut(&mut planned.result.clone()).len(), 1);
    }
}

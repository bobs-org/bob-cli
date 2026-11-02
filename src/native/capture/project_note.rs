//! Project-note planning for plus-sigil captures.
use super::*;

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
    // The grammar rejects a Create `#pomodoro` name with no ` :` task
    // before planning; this guards planners that bypass it. Offer defers
    // the check until after the pre-pick render, so a name is valid when
    // the note has an open task to offer.
    if form == ProjectNoteForm::Create
        && let Some(name) = pomodoro_name.as_deref()
    {
        let has_link = parsed.sub_bullets.iter().any(|sub_bullet| {
            sub_bullet.task_id.as_ref().is_some_and(|task| task.link)
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

    let mut frozen_offers: Vec<FrozenOffer> = Vec::new();
    let mut project_folds: Vec<crate::native::task_complete::ProjectFold> =
        Vec::new();
    if form == ProjectNoteForm::Offer {
        let already_planned =
            rendered.task_rows.iter().filter(|row| row.link).count();
        let hidden = rendered
            .task_rows
            .iter()
            .filter(|row| !row.placeholder && row.hidden && !row.link)
            .count();
        let offer_rows: Vec<&capture_project_note::RenderedTask> =
            if project_blocked {
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
        let has_colon = parsed.sub_bullets.iter().any(|sub_bullet| {
            sub_bullet.task_id.as_ref().is_some_and(|task| task.link)
        });
        if let Some(name) = pomodoro_name.as_deref()
            && offer_rows.is_empty()
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
                    relative.clone(),
                    0,
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
                settled_ordinal: None,
            });
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
            for offer in frozen_offers.iter_mut().filter(|offer| offer.selected)
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

    let project_link_guard: Option<LinkGuardJson> = if project_blocked
        && (form == ProjectNoteForm::Offer
            || rendered.named_tasks.iter().any(|task| task.link))
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
    let linked: Vec<capture_project_note::RenderedNamedTask> = rendered
        .named_tasks
        .iter()
        .filter(|task| task.link && !project_blocked)
        .cloned()
        .collect();
    let note_route: &str = basename.strip_suffix(".md").unwrap_or(&basename);
    // Stage the new note before linking so per-task type resolution (D3)
    // sees the staged residence, including the just-rendered task lines.
    planner.stage(&new_target, rendered.contents.clone())?;
    let need_day = !linked.is_empty() || !frozen_offers.is_empty();
    let mut actual_placements: Vec<PlacedProjectLink> = Vec::new();
    let mut scratch_placements: Vec<PlacedProjectLink> = Vec::new();
    let (
        day_file,
        pomodoro_link_placement,
        mut canonical_pomodoro_name,
        creates_pomodoro,
        mut first_link_destination,
        day_relative,
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

    let prj_text = parsed.body.trim().to_string();
    Ok(PlannedCaptureItem {
        result: CaptureItemResult {
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
                form: (form == ProjectNoteForm::Offer).then_some("offer"),
                successors: (form == ProjectNoteForm::Offer).then(|| {
                    let mut scratch_index = 0;
                    frozen_offers
                        .iter()
                        .map(|offer| {
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
                            }
                        })
                        .collect()
                }),
                project_folds,
            }),
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_close: None,
            pomodoro_reset: None,
            dependency_update: None,
            consumed_links: Vec::new(),
            task_complete: None,
            toggle_task_description: None,
            r#ref: None,
            link_guard: project_link_guard,
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
        pomodoro_refs: project_merges
            .iter()
            .map(|merge| {
                super::PomodoroBlockRef::at(
                    super::PomodoroBlockRole::Changed,
                    merge.dest_index,
                    merge.dest_index,
                )
            })
            .collect(),
        task_block_refs: Vec::new(),
    })
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
        OfferHarness {
            planner: CaptureBatchPlanner::default(),
            task_theme_context: CaptureTaskThemeContext::new(
                &request.bob_dir,
                today,
                None,
            ),
            warnings: Vec::new(),
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

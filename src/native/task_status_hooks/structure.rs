//! Structural duplicate and list-item plans for Pomodoro bullets.
use super::*;

pub(crate) fn fenced_lines(
    lines: &[&str],
    section: Range<usize>,
) -> BTreeSet<usize> {
    super::super::markdown::fenced_lines(lines, section)
}

pub(super) fn entry_block_end(
    lines: &[&str],
    entry_line: usize,
    section_end: usize,
) -> usize {
    let mut index = entry_line + 1;
    while index < section_end {
        if leading_indentation_len(lines[index]) > 0
            || (lines[index].trim().is_empty()
                && next_nonblank_is_indented(lines, index + 1, section_end))
        {
            index += 1;
        } else {
            break;
        }
    }
    index
}

pub(super) fn bullet_block_end(
    lines: &[&str],
    bullet_line: usize,
    entry_end: usize,
    indentation: usize,
) -> usize {
    let mut index = bullet_line + 1;
    while index < entry_end {
        let line = lines[index];
        if leading_indentation_len(line) > indentation
            || (line.trim().is_empty()
                && next_nonblank_more_indented(
                    lines,
                    index + 1,
                    entry_end,
                    indentation,
                ))
        {
            index += 1;
        } else {
            break;
        }
    }
    index
}

pub(super) fn leading_indentation_len(line: &str) -> usize {
    line.as_bytes()
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count()
}

pub(super) fn next_nonblank_is_indented(
    lines: &[&str],
    start: usize,
    end: usize,
) -> bool {
    lines[start..end]
        .iter()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| leading_indentation_len(line) > 0)
}

pub(super) fn next_nonblank_more_indented(
    lines: &[&str],
    start: usize,
    end: usize,
    indentation: usize,
) -> bool {
    lines[start..end]
        .iter()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| leading_indentation_len(line) > indentation)
}

pub(super) fn is_done_status(
    status: char,
    done_statuses: &BTreeSet<char>,
) -> bool {
    matches!(status, 'x' | 'X') || done_statuses.contains(&status)
}

pub(super) fn is_canceled_status(
    status: char,
    status_types: &BTreeMap<char, TaskStatusType>,
) -> bool {
    status_types.get(&status) == Some(&TaskStatusType::Cancelled)
}

pub(super) fn plan_duplicate_line_removals(
    lines: &[&str],
    model: &PomodoroModel,
    resolved: &BTreeMap<RawReference, ResolvedReference>,
) -> Vec<RemovedDuplicateLine> {
    let mut owners: BTreeMap<(PathBuf, String), usize> = BTreeMap::new();
    let mut removed = Vec::new();

    for bullet in &model.bullets {
        let entry = &model.entries[bullet.entry_index];
        if !entry.open {
            continue;
        }
        let tasks = bullet
            .links
            .iter()
            .filter_map(|link| {
                resolved.get(&link.reference).map(|reference| {
                    (reference.path.clone(), link.reference.block_id.clone())
                })
            })
            .collect::<BTreeSet<_>>();
        if tasks.is_empty() {
            continue;
        }

        let duplicate_tasks = tasks
            .iter()
            .filter(|task| {
                owners
                    .get(*task)
                    .is_some_and(|owner| *owner != bullet.entry_index)
            })
            .map(|(path, block_id)| DuplicateTaskIdentity {
                path: display_path(path),
                block_id: block_id.clone(),
            })
            .collect::<Vec<_>>();
        if !duplicate_tasks.is_empty() {
            removed.push(RemovedDuplicateLine {
                line_number: bullet.line_index + 1,
                pomodoro: entry.context.clone(),
                line: lines[bullet.line_index].to_string(),
                duplicate_tasks,
            });
            continue;
        }

        for task in tasks {
            owners.entry(task).or_insert(bullet.entry_index);
        }
    }

    removed
}

pub(crate) fn plan_structural_changes(
    model: &PomodoroModel,
    resolved: &BTreeMap<RawReference, ResolvedReference>,
    done_statuses: &BTreeSet<char>,
    status_types: &BTreeMap<char, TaskStatusType>,
    duplicate_deleted_lines: &BTreeSet<usize>,
    dedupe_into_destination: bool,
) -> StructuralPlan {
    plan_structural_changes_with_blocked(
        model,
        resolved,
        done_statuses,
        status_types,
        duplicate_deleted_lines,
        dedupe_into_destination,
        &[],
        &BTreeSet::new(),
    )
}

/// Like [`plan_structural_changes`], with an explicit Blocked identity set
/// (`docs/task-dependencies.md` §13.5). Never keys consumption off `'?'`:
/// `task_complete` retirement passes an empty set so `Live{'?'}` stays.
pub(crate) fn plan_structural_changes_with_blocked(
    model: &PomodoroModel,
    resolved: &BTreeMap<RawReference, ResolvedReference>,
    done_statuses: &BTreeSet<char>,
    status_types: &BTreeMap<char, TaskStatusType>,
    duplicate_deleted_lines: &BTreeSet<usize>,
    dedupe_into_destination: bool,
    lines: &[&str],
    blocked: &BTreeSet<(PathBuf, String)>,
) -> StructuralPlan {
    let current = model
        .entries
        .iter()
        .position(|entry| entry.open && entry.timed && entry.has_child);
    let fallback = model
        .entries
        .iter()
        .rposition(|entry| entry.completed && entry.has_child);
    let target_entry = current.or(fallback);
    // With the opt-in dedupe flag, a bullet that would move is deleted
    // instead when the destination entry already links the same task
    // (struck or not). Reconcile passes `false` and is unaffected.
    let destination_identities: BTreeSet<(PathBuf, String)> =
        if dedupe_into_destination {
            target_entry.map_or(BTreeSet::new(), |target| {
                model
                    .bullets
                    .iter()
                    .filter(|bullet| bullet.entry_index == target)
                    .flat_map(|bullet| bullet.links.iter())
                    .filter_map(|link| {
                        resolved.get(&link.reference).map(|reference| {
                            (
                                reference.path.clone(),
                                link.reference.block_id.clone(),
                            )
                        })
                    })
                    .collect()
            })
        } else {
            BTreeSet::new()
        };
    let mut token_edits: BTreeMap<usize, Vec<TokenEdit>> = BTreeMap::new();
    let mut move_candidates = Vec::new();
    let mut struck = Vec::new();
    let mut moved = Vec::new();
    let mut deduplicated = Vec::new();
    let mut marker_added = Vec::new();
    let mut marker_removed = Vec::new();
    let mut removed_canceled = Vec::new();
    let mut deleted_lines = duplicate_deleted_lines.clone();

    // Cancellation owns the complete list-item subtree. Plan it before any
    // token edits or moves so descendants and sibling links that will be
    // deleted cannot produce additional structural work or reports.
    for bullet in &model.bullets {
        if deleted_lines.contains(&bullet.line_index) {
            continue;
        }
        let source = &model.entries[bullet.entry_index];
        let canceled_links = bullet
            .links
            .iter()
            .filter(|link| {
                source.open
                    && resolved.get(&link.reference).is_some_and(|reference| {
                        !reference.statuses.is_empty()
                            && reference.statuses.iter().all(|status| {
                                is_canceled_status(*status, status_types)
                            })
                    })
            })
            .collect::<Vec<_>>();
        if canceled_links.is_empty() {
            continue;
        }
        for link in canceled_links {
            removed_canceled.push(RemovedCanceledReference {
                target: link.reference.target.clone(),
                block_id: link.reference.block_id.clone(),
                line_number: bullet.line_index + 1,
                pomodoro: source.context.clone(),
            });
        }
        deleted_lines.extend(bullet.line_index..bullet.end_line);
    }

    let (blocked_edits, blocked_deleted, removed_blocked) =
        plan_blocked_link_removals(
            model,
            resolved,
            lines,
            blocked,
            &deleted_lines,
        );
    deleted_lines.extend(blocked_deleted);
    for (line_index, edits) in blocked_edits {
        token_edits.entry(line_index).or_default().extend(edits);
    }

    for bullet in &model.bullets {
        if deleted_lines.contains(&bullet.line_index) {
            continue;
        }
        let source = &model.entries[bullet.entry_index];
        let completed_links = bullet
            .links
            .iter()
            .filter(|link| {
                resolved.get(&link.reference).is_some_and(|reference| {
                    !reference.statuses.is_empty()
                        && reference.statuses.iter().all(|status| {
                            is_done_status(*status, done_statuses)
                        })
                })
            })
            .collect::<Vec<_>>();
        let move_target = if !completed_links.is_empty() && source.open {
            target_entry.filter(|target| {
                if *target == bullet.entry_index {
                    return false;
                }
                let has_live_reference = bullet.links.iter().any(|link| {
                    resolved.get(&link.reference).is_some_and(|reference| {
                        reference.statuses.iter().any(|status| {
                            !is_done_status(*status, done_statuses)
                        })
                    })
                });
                model.entries[*target].open || !has_live_reference
            })
        } else {
            None
        };
        if dedupe_into_destination
            && let Some(target) = move_target
            && bullet.end_line == bullet.line_index + 1
            && !bullet.links.is_empty()
            && bullet.links.iter().all(|link| {
                resolved.get(&link.reference).is_some_and(|reference| {
                    destination_identities.contains(&(
                        reference.path.clone(),
                        link.reference.block_id.clone(),
                    ))
                })
            })
        {
            deleted_lines.extend(bullet.line_index..bullet.end_line);
            deduplicated.push(DeduplicatedCompletedReference {
                target: bullet.links[0].reference.target.clone(),
                block_id: bullet.links[0].reference.block_id.clone(),
                source_pomodoro: source.context.clone(),
                destination_pomodoro: model.entries[target].context.clone(),
            });
            continue;
        }
        let final_entry = move_target.unwrap_or(bullet.entry_index);
        for link in &bullet.links {
            if resolved.get(&link.reference).is_some_and(|reference| {
                blocked.contains(&(
                    reference.path.clone(),
                    link.reference.block_id.clone(),
                ))
            }) {
                continue;
            }
            let retire = completed_links
                .iter()
                .any(|completed| std::ptr::eq(*completed, link));
            let marker_expected = marker_expected_for_occurrence(
                &model.entries[final_entry],
                link,
            );
            let replacement = desired_link_token(link, retire, marker_expected);
            if link.current_token != replacement {
                token_edits.entry(bullet.line_index).or_default().push(
                    TokenEdit {
                        start: link.edit_start,
                        end: link.edit_end,
                        replacement: replacement.to_string(),
                    },
                );
            }
            if retire && (!link.struck || link.embedded) {
                struck.push(StruckCompletedReference {
                    target: link.reference.target.clone(),
                    block_id: link.reference.block_id.clone(),
                    pomodoro: source.context.clone(),
                    removed_embed: link.embedded,
                });
            }

            let desired_marker_count = usize::from(marker_expected);
            if link.marker_count != desired_marker_count {
                let item = MarkerReference {
                    target: link.reference.target.clone(),
                    block_id: link.reference.block_id.clone(),
                    pomodoro: model.entries[final_entry].context.clone(),
                };
                if link.marker_count < desired_marker_count {
                    marker_added.push(item);
                } else {
                    marker_removed.push(item);
                }
            }
        }

        if let Some(target) = move_target {
            move_candidates.push(BulletMove {
                start_line: bullet.line_index,
                end_line: bullet.end_line,
                source_indentation: bullet.indentation.clone(),
            });
            for link in completed_links {
                moved.push(MovedCompletedReference {
                    target: link.reference.target.clone(),
                    block_id: link.reference.block_id.clone(),
                    source_pomodoro: source.context.clone(),
                    destination_pomodoro: model.entries[target].context.clone(),
                });
            }
        }
    }

    move_candidates.sort_by_key(|item| (item.start_line, item.end_line));
    let mut moves: Vec<BulletMove> = Vec::new();
    for candidate in move_candidates {
        if moves
            .last()
            .is_some_and(|previous| candidate.start_line < previous.end_line)
        {
            continue;
        }
        moves.push(candidate);
    }

    StructuralPlan {
        token_edits,
        moves,
        deleted_lines,
        target_entry,
        struck,
        moved,
        deduplicated,
        marker_added,
        marker_removed,
        removed_canceled,
        removed_blocked,
    }
}

/// Plan live-link consumption for `blocked` identities under open
/// Pomodoros. Does not retire completed links or repair `🍅` markers:
/// the second rewrite pass applies only these edits plus empty
/// Pomodoros (`docs/task-dependencies.md` §13.5).
fn plan_blocked_link_removals(
    model: &PomodoroModel,
    resolved: &BTreeMap<RawReference, ResolvedReference>,
    lines: &[&str],
    blocked: &BTreeSet<(PathBuf, String)>,
    already_deleted: &BTreeSet<usize>,
) -> (
    BTreeMap<usize, Vec<TokenEdit>>,
    BTreeSet<usize>,
    Vec<RemovedBlockedReference>,
) {
    let mut token_edits: BTreeMap<usize, Vec<TokenEdit>> = BTreeMap::new();
    let mut deleted_lines = BTreeSet::new();
    let mut removed_blocked = Vec::new();
    if blocked.is_empty() {
        return (token_edits, deleted_lines, removed_blocked);
    }
    for bullet in &model.bullets {
        if already_deleted.contains(&bullet.line_index)
            || deleted_lines.contains(&bullet.line_index)
        {
            continue;
        }
        let source = &model.entries[bullet.entry_index];
        if !source.open {
            continue;
        }
        let blocked_links = bullet
            .links
            .iter()
            .filter(|link| {
                !link.struck
                    && resolved.get(&link.reference).is_some_and(|reference| {
                        blocked.contains(&(
                            reference.path.clone(),
                            link.reference.block_id.clone(),
                        ))
                    })
            })
            .collect::<Vec<_>>();
        if blocked_links.is_empty() {
            continue;
        }
        let line = lines.get(bullet.line_index).copied().unwrap_or("");
        for link in &blocked_links {
            let resolved_path = resolved
                .get(&link.reference)
                .map(|reference| reference.path.clone())
                .expect("blocked links are resolved");
            removed_blocked.push(RemovedBlockedReference {
                target: link.reference.target.clone(),
                block_id: link.reference.block_id.clone(),
                line_number: bullet.line_index + 1,
                pomodoro: source.context.clone(),
                task_path: resolved_path,
            });
        }
        // A link-only bullet (body is exactly one blocked live token)
        // goes with its subtree; otherwise only each blocked token
        // is removed (PDL1, PDL6, PDL7).
        let link_only = blocked_links
            .iter()
            .any(|link| is_link_only_bullet(line, link));
        if link_only {
            deleted_lines.extend(bullet.line_index..bullet.end_line);
        } else {
            for link in blocked_links {
                token_edits.entry(bullet.line_index).or_default().push(
                    TokenEdit {
                        start: link.edit_start,
                        end: link.edit_end,
                        replacement: String::new(),
                    },
                );
            }
        }
    }
    (token_edits, deleted_lines, removed_blocked)
}

fn is_link_only_bullet(line: &str, link: &LinkOccurrence) -> bool {
    let indent = leading_indentation_len(line);
    let Some(after) = after_list_marker(line, indent) else {
        return false;
    };
    let body = line[after..].trim();
    let token = line
        .get(link.edit_start..link.edit_end)
        .unwrap_or("")
        .trim();
    !token.is_empty() && body == token
}

/// Consume live links to `blocked` identities under today's open
/// Pomodoros, then run the empty-Pomodoro rule against the matching
/// pre-consumption model (`docs/task-dependencies.md` §13.5).
pub(crate) struct BlockedConsumption {
    pub(crate) contents: String,
    pub(crate) consumed: Vec<RemovedBlockedReference>,
    pub(crate) removed_empty: Vec<RemovedEmptyPomodoro>,
}

pub(crate) fn apply_blocked_consumption(
    contents: &str,
    resolved: &BTreeMap<RawReference, ResolvedReference>,
    blocked: &BTreeSet<(PathBuf, String)>,
) -> BlockedConsumption {
    let unchanged = BlockedConsumption {
        contents: contents.to_string(),
        consumed: Vec::new(),
        removed_empty: Vec::new(),
    };
    if blocked.is_empty() {
        return unchanged;
    }
    let lines = logical_lines(contents);
    let Some(section) = native_pomodoro::pomodoros_section_range(&lines) else {
        return unchanged;
    };
    let model = scan_pomodoros(&lines, section);
    let (token_edits, deleted_lines, consumed) = plan_blocked_link_removals(
        &model,
        resolved,
        &lines,
        blocked,
        &BTreeSet::new(),
    );
    if consumed.is_empty() {
        return unchanged;
    }
    let plan = StructuralPlan {
        token_edits,
        moves: Vec::new(),
        deleted_lines,
        target_entry: None,
        struck: Vec::new(),
        moved: Vec::new(),
        deduplicated: Vec::new(),
        marker_added: Vec::new(),
        marker_removed: Vec::new(),
        removed_canceled: Vec::new(),
        removed_blocked: consumed,
    };
    let updated = apply_structural_plan(contents, &model, &plan);
    let empty_plan = plan_empty_pomodoro_removals(&updated, &model);
    let text = apply_empty_pomodoro_plan(&updated, &empty_plan);
    BlockedConsumption {
        contents: text,
        consumed: plan.removed_blocked,
        removed_empty: empty_plan.removed,
    }
}

pub(crate) fn apply_structural_plan(
    contents: &str,
    model: &PomodoroModel,
    plan: &StructuralPlan,
) -> String {
    if plan.token_edits.is_empty()
        && plan.moves.is_empty()
        && plan.deleted_lines.is_empty()
    {
        return contents.to_string();
    }
    let mut lines = contents
        .split_inclusive('\n')
        .map(str::to_string)
        .collect::<Vec<_>>();
    for (line_index, edits) in &plan.token_edits {
        let mut edits = edits.iter().collect::<Vec<_>>();
        edits.sort_by_key(|edit| edit.start);
        for edit in edits.into_iter().rev() {
            lines[*line_index]
                .replace_range(edit.start..edit.end, &edit.replacement);
        }
    }

    let target_indentation = plan.target_entry.map(|target| {
        model.entries[target]
            .child_indentation
            .clone()
            .or_else(|| {
                model
                    .entries
                    .iter()
                    .find_map(|entry| entry.child_indentation.clone())
            })
            .unwrap_or_else(|| "  ".to_string())
    });
    let mut removed = plan.deleted_lines.clone();
    let mut moved_lines = Vec::new();
    for item in &plan.moves {
        let target_indentation = target_indentation
            .as_deref()
            .expect("moves always have a target Pomodoro");
        for (index, line) in lines
            .iter()
            .enumerate()
            .take(item.end_line)
            .skip(item.start_line)
        {
            removed.insert(index);
            if !plan.deleted_lines.contains(&index) {
                moved_lines.push(reindent_segment(
                    line,
                    &item.source_indentation,
                    target_indentation,
                ));
            }
        }
    }
    let insertion_line = plan
        .target_entry
        .map(|target| model.entries[target].end_line);
    let ending = if contents.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut output =
        String::with_capacity(contents.len() + plan.struck.len() * 4);
    for index in 0..=lines.len() {
        if insertion_line == Some(index) && !moved_lines.is_empty() {
            if !output.is_empty() && !output.ends_with('\n') {
                output.push_str(ending);
            }
            for line in &moved_lines {
                output.push_str(line);
            }
            if index < lines.len() && !output.ends_with('\n') {
                output.push_str(ending);
            }
        }
        if index < lines.len() && !removed.contains(&index) {
            output.push_str(&lines[index]);
        }
    }
    output
}

pub(super) fn reindent_segment(
    segment: &str,
    source_indentation: &str,
    target_indentation: &str,
) -> String {
    let line = logical_line(segment);
    if line.is_empty() || !line.starts_with(source_indentation) {
        return segment.to_string();
    }
    let ending = &segment[line.len()..];
    format!(
        "{target_indentation}{}{ending}",
        &line[source_indentation.len()..]
    )
}

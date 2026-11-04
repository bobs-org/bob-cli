//! Complete pure task-merge planning over one vault snapshot.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use super::super::{
    capture_task_toggle::set_task_line_status,
    freshness::stamp_fresh,
    note_tasks,
    pomodoro::pomodoros_section_range,
    task_cancel::{self, TaskIdentity},
    task_complete::successors::{
        self as successors, DayView, SuccessorPlacement, SuccessorTarget,
    },
    task_locator::{self, LocatedTask},
    task_move::VaultSnapshot,
    task_status_hooks::{logical_lines, scan_pomodoros},
    vault_links::LinkResolution,
};
use super::{
    containers,
    payload::{prepare_task_merge, validate_merge_request},
    retirement::{
        self, classify_source, normalize_merge_label, RetirementReports,
        SourceOutcome,
    },
    types::{
        DailyNoteReadState, MergeConsequence, MergeContainer, MergeContext,
        MergeError, MergeGesture, MergeOffer, MergeOutcome, MergePlan,
        MergeRecord, MergeRequest, MergeTargetDescriptor, MergeTaskDescriptor,
        PreparedMerge,
    },
};

#[derive(Debug, Clone)]
struct SourceState {
    located: LocatedTask,
    outcome: SourceOutcome,
}

/// Compose payload, dependency retirement, cancellation recovery, and a
/// single ledger offer into one all-or-nothing postimage set.
pub(crate) fn plan_task_merge(
    snapshot: &VaultSnapshot,
    request: &MergeRequest,
    context: &MergeContext<'_>,
) -> Result<MergePlan, MergeError> {
    if request.sources.is_empty() {
        return Err(MergeError::new(
            "merge_no_sources",
            "select at least one source task",
        ));
    }
    let label = normalize_merge_label(&request.label)?;
    validate_merge_request(snapshot, request, context)?;
    let target_identity = target_identity(snapshot, &request.target, context)?;

    let mut sources = Vec::with_capacity(request.sources.len());
    for locator in &request.sources {
        let located = containers::resolve_task(snapshot, locator, context)?;
        let (outcome, _) = classify_source(
            snapshot,
            locator,
            &request.target,
            &label,
            context,
        )?;
        if request.gesture == MergeGesture::Explicit
            && outcome == SourceOutcome::AlreadyClosed
        {
            return Err(MergeError::new(
                "merge_source_closed",
                format!("^{} is closed", locator.block_id),
            ));
        }
        if target_identity.as_ref()
            == Some(&(
                located.path.clone(),
                located.task.block_id.clone().unwrap_or_default(),
            ))
        {
            return Err(MergeError::new(
                "merge_target_is_source",
                format!("^{} cannot be merged into itself", locator.block_id),
            ));
        }
        sources.push(SourceState { located, outcome });
    }

    // The public planner never cuts an unchanged or label-only closed source.
    let new_locators = request
        .sources
        .iter()
        .zip(&sources)
        .filter_map(|(locator, source)| {
            (source.outcome == SourceOutcome::New).then_some(locator.clone())
        })
        .collect::<Vec<_>>();
    let new_request = MergeRequest {
        sources: new_locators,
        ..request.clone()
    };
    let prepared = if new_request.sources.is_empty() {
        None
    } else {
        Some(prepare_task_merge(snapshot, &new_request, context)?)
    };
    let target = target_descriptor(
        snapshot,
        &request.target,
        prepared.as_ref(),
        context,
    )?;

    let active_sources = sources
        .iter()
        .filter(|source| source.outcome == SourceOutcome::New)
        .map(|source| source_identity(&source.located))
        .collect::<BTreeSet<_>>();
    // Slot identities are collected from the staged daily note before any
    // source cancellation prunes its live Task Links.
    let pre_prune_day = if active_sources.is_empty() {
        None
    } else {
        daily_preimage(snapshot, prepared.as_ref(), context)?
    };
    let note_index = context
        .note_index
        .with_additional(snapshot.notes.keys().cloned());
    let prune = task_cancel::prune::prune_daily(
        &context.daily_path,
        pre_prune_day.as_deref(),
        &active_sources,
        &note_index,
    );

    let (mut staged, retirement_reports) = if let Some(prepared) = &prepared {
        retirement::apply_retirement(
            snapshot,
            prepared,
            &new_request,
            target_identity.clone(),
            context,
        )?
    } else {
        (snapshot.notes.clone(), Vec::new())
    };

    // A label gesture on a closed source only adds the relation. It never
    // enters payload, cancellation, recovery, or ledger planning.
    if request.gesture == MergeGesture::Label {
        if let Some(target_identity) = target_identity.as_ref() {
            for source in &sources {
                if source.outcome == SourceOutcome::AlreadyClosed {
                    if !target_is_open(&staged, target_identity, context)? {
                        return Err(MergeError::new(
                            "merge_target_closed",
                            format!("^{} is closed", target_identity.1),
                        ));
                    }
                    retirement::stage_task_provenance(
                        &mut staged,
                        &source_relation_identity(&source.located),
                        target_identity,
                        &label,
                        context,
                    )?;
                }
            }
        } else if sources
            .iter()
            .any(|source| source.outcome == SourceOutcome::AlreadyClosed)
        {
            return Err(MergeError::new(
                "merge_section_closed_label",
                "a closed source can only receive a task merge relation",
            ));
        }
    }

    let mut records = build_records(
        &sources,
        prepared.as_ref(),
        &retirement_reports,
        &target,
        &label,
        context,
        &staged,
    );
    let mut warnings = prepared
        .as_ref()
        .map(|prepared| prepared.warnings.clone())
        .unwrap_or_default();
    warnings.extend(
        retirement_reports
            .iter()
            .flat_map(|report| report.warnings.iter().cloned()),
    );

    if let Some(target_identity) = target_identity.as_ref()
        && !prune.removed_slots.is_empty()
        && let Some(day_text) =
            staged.get(&context.daily_path).or(prune.content.as_ref())
        && !successors::live_link_keys(
            day_text,
            &context.daily_path,
            &note_index,
        )
        .contains(target_identity)
        && target_can_hold_link(&staged, target_identity, context)?
    {
        let slot = prune
            .removed_slots
            .iter()
            .filter(|slot| active_sources.contains(&slot.task))
            .min_by_key(|slot| {
                (
                    slot.entry_index,
                    slot.original_line,
                    slot.task.path.clone(),
                    slot.task.block_id.clone(),
                )
            });
        if let Some(slot) = slot {
            let Some((slot_entry_index, _)) =
                rebase_pruned_slot(day_text, slot)
            else {
                return Err(MergeError::new(
                    "merge_offer_slot_missing",
                    "the removed ledger link slot could not be found after staging",
                ));
            };
            let day_view =
                DayView::parse(day_text, &context.daily_path, &note_index);
            if let Some(entry) = day_view
                .entries
                .get(slot_entry_index)
                .filter(|entry| entry.open)
            {
                let located_target =
                    resolve_identity(&staged, target_identity, context)?;
                let key = task_key(&located_target, context);
                let selected = request.selection.selects(&key);
                let source = sources
                    .iter()
                    .find(|source| {
                        source_identity(&source.located) == slot.task
                    })
                    .expect("pruned slot belongs to an active source");
                let offer = MergeOffer {
                    key,
                    n: 1,
                    group: "hand_off".to_string(),
                    selected,
                    entry_name: entry.name.clone(),
                    entry_role: "slot".to_string(),
                    merged_from: Some(task_key(&source.located, context)),
                };
                if selected {
                    apply_picked_target(&mut staged, target_identity, context)?;
                    let day_text = staged
                        .get(&context.daily_path)
                        .or(prune.content.as_ref())
                        .map(String::as_str)
                        .unwrap_or_default();
                    let Some((entry_index, line_index)) =
                        rebase_pruned_slot(day_text, slot)
                    else {
                        return Err(MergeError::new(
                            "merge_offer_slot_missing",
                            "the removed ledger link slot could not be found after target edits",
                        ));
                    };
                    let link = successors::successor_link_text(
                        &target_identity.0,
                        &target_identity.1,
                        &context.daily_path,
                        &note_index,
                    );
                    let (updated, _) = successors::insert_successor_links(
                        day_text,
                        &context.daily_path,
                        &note_index,
                        vec![SuccessorPlacement {
                            target: SuccessorTarget::ExactSlot {
                                entry_index,
                                line_index,
                                indent: slot.indentation.clone(),
                            },
                            block_link: link,
                        }],
                    );
                    staged.insert(context.daily_path.clone(), updated);
                }
                let source_identity = source_identity(&source.located);
                if let Some(record) = records.iter_mut().find(|record| {
                    record.source.path == source_identity.path
                        && record.source.block_id == source_identity.block_id
                }) {
                    record.offers.push(offer);
                }
            }
        }
    }

    enrich_target_lines(&mut records, &staged, context);
    let postimages = staged
        .into_iter()
        .filter(|(path, contents)| snapshot.notes.get(path) != Some(contents))
        .collect::<BTreeMap<_, _>>();
    let mut read_paths = prepared
        .as_ref()
        .map(|prepared| prepared.read_paths.clone())
        .unwrap_or_else(|| snapshot.notes.keys().cloned().collect());
    read_paths.insert(context.daily_path.clone());
    let renamed_block_ids = prepared
        .as_ref()
        .map(|prepared| prepared.renamed_block_ids.clone())
        .unwrap_or_default();
    let repaired_files = prepared
        .as_ref()
        .map(|prepared| prepared.repaired_files.clone())
        .unwrap_or_default();
    let write_order = ordered_paths(
        &postimages,
        &sources,
        &target.path,
        &retirement_reports,
        &repaired_files,
        &context.daily_path,
    );
    warnings.sort();
    warnings.dedup();
    Ok(MergePlan {
        postimages,
        write_order,
        records,
        renamed_block_ids,
        repaired_files,
        warnings,
        read_paths,
    })
}

/// Rebase a pruned slot onto the final staged daily note using its ledger
/// header identity and the count of retained linked bullets before it.
fn rebase_pruned_slot(
    day_text: &str,
    slot: &task_cancel::prune::PrunedLinkSlot,
) -> Option<(usize, usize)> {
    let lines = logical_lines(day_text);
    let section = pomodoros_section_range(&lines)?;
    let model = scan_pomodoros(&lines, section);
    let entry_index = model
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.context == slot.entry_context)
        .nth(slot.entry_ordinal)
        .map(|(index, _)| index)?;
    let entry = model.entries.get(entry_index)?;
    if !entry.open {
        return None;
    }
    let line_index =
        (entry.line_index + 1 + slot.retained_lines_before).min(entry.end_line);
    Some((entry_index, line_index))
}

fn target_descriptor(
    snapshot: &VaultSnapshot,
    target: &MergeContainer,
    prepared: Option<&PreparedMerge>,
    context: &MergeContext<'_>,
) -> Result<MergeTargetDescriptor, MergeError> {
    match target {
        MergeContainer::Task(locator) | MergeContainer::Drawer(locator, _) => {
            let located = containers::resolve_task(snapshot, locator, context)?;
            let (kind, drawer, created) = match target {
                MergeContainer::Task(_) => ("task", None, false),
                MergeContainer::Drawer(_, selector) => {
                    let (title, created) = prepared
                        .and_then(|prepared| prepared.container.as_ref())
                        .and_then(|container| match &container.kind {
                            super::types::PreparedContainerKind::Drawer {
                                title,
                                created,
                            } => Some((title.clone(), *created)),
                            _ => None,
                        })
                        .map_or_else(
                            || (selector.text.clone(), false),
                            |(title, created)| (title, created),
                        );
                    ("drawer", Some(title), created)
                }
                MergeContainer::Section { .. } => unreachable!(),
            };
            Ok(MergeTargetDescriptor {
                path: located.path,
                block_id: located.task.block_id,
                title: Some(note_tasks::clean_description(
                    &located.task.description,
                    &context.task_settings.global_filter,
                    None,
                )),
                drawer,
                section: None,
                kind: kind.to_string(),
                line: None,
                created,
                status_symbol: Some(located.task.status_symbol),
                status_name: Some(located.task.status_name.clone()),
            })
        }
        MergeContainer::Section { note, selector } => {
            let index = context
                .note_index
                .with_additional(snapshot.notes.keys().cloned());
            let path = match index.resolve_detailed(None, note) {
                LinkResolution::Found(path) => path,
                LinkResolution::Ambiguous => {
                    return Err(MergeError::new(
                        "ambiguous_note",
                        format!("`{note}` matches more than one note"),
                    ));
                }
                LinkResolution::Missing => {
                    return Err(MergeError::new(
                        "task_not_found",
                        format!("no note matches `{note}`"),
                    ));
                }
            };
            let (title, created) = prepared
                .and_then(|prepared| prepared.container.as_ref())
                .and_then(|container| match &container.kind {
                    super::types::PreparedContainerKind::Section {
                        title,
                        created,
                    } => Some((title.clone(), *created)),
                    _ => None,
                })
                .map_or_else(
                    || (selector.text.clone(), false),
                    |(title, created)| (title, created),
                );
            Ok(MergeTargetDescriptor {
                path,
                block_id: None,
                title: None,
                drawer: None,
                section: Some(title),
                kind: "section".to_string(),
                line: None,
                created,
                status_symbol: None,
                status_name: None,
            })
        }
    }
}

fn target_identity(
    snapshot: &VaultSnapshot,
    target: &MergeContainer,
    context: &MergeContext<'_>,
) -> Result<Option<(PathBuf, String)>, MergeError> {
    let locator = match target {
        MergeContainer::Task(locator) | MergeContainer::Drawer(locator, _) => {
            locator
        }
        MergeContainer::Section { .. } => return Ok(None),
    };
    let located = containers::resolve_task(snapshot, locator, context)?;
    Ok(Some((
        located.path,
        located.task.block_id.unwrap_or_default(),
    )))
}

fn source_identity(located: &LocatedTask) -> TaskIdentity {
    TaskIdentity {
        path: located.path.clone(),
        block_id: located.task.block_id.clone().unwrap_or_default(),
    }
}

fn source_relation_identity(located: &LocatedTask) -> (PathBuf, String) {
    (
        located.path.clone(),
        located.task.block_id.clone().unwrap_or_default(),
    )
}

fn resolve_identity(
    notes: &BTreeMap<PathBuf, String>,
    identity: &(PathBuf, String),
    context: &MergeContext<'_>,
) -> Result<LocatedTask, MergeError> {
    task_locator::resolve_task(
        notes,
        &format!("{}:{}", identity.0.display(), identity.1),
        context.task_settings,
    )
    .map_err(|error| MergeError::new(error.code, error.message))
}

fn target_is_open(
    notes: &BTreeMap<PathBuf, String>,
    identity: &(PathBuf, String),
    context: &MergeContext<'_>,
) -> Result<bool, MergeError> {
    Ok(resolve_identity(notes, identity, context)?
        .task
        .status_type
        .is_open())
}

fn target_can_hold_link(
    notes: &BTreeMap<PathBuf, String>,
    identity: &(PathBuf, String),
    context: &MergeContext<'_>,
) -> Result<bool, MergeError> {
    let task = resolve_identity(notes, identity, context)?;
    Ok(task.task.status_type.is_open() && task.task.status_symbol != '?')
}

fn task_key(located: &LocatedTask, context: &MergeContext<'_>) -> String {
    note_tasks::stable_task_key(
        &located.path,
        located.task.block_id.as_deref(),
        &located.task.description,
        context.task_settings.global_filter.as_str(),
        1,
    )
}

fn daily_preimage(
    snapshot: &VaultSnapshot,
    prepared: Option<&PreparedMerge>,
    context: &MergeContext<'_>,
) -> Result<Option<String>, MergeError> {
    let staged = prepared
        .and_then(|prepared| prepared.postimages.get(&context.daily_path))
        .cloned();
    match &context.daily_read {
        DailyNoteReadState::Failed(error) => {
            Err(MergeError::new("merge_daily_read_failed", error.clone()))
        }
        DailyNoteReadState::Missing => Ok(None),
        DailyNoteReadState::Read(text) => Ok(Some(
            staged
                .or_else(|| snapshot.notes.get(&context.daily_path).cloned())
                .unwrap_or_else(|| text.clone()),
        )),
        DailyNoteReadState::NotRead => {
            Ok(staged
                .or_else(|| snapshot.notes.get(&context.daily_path).cloned()))
        }
    }
}

fn apply_picked_target(
    staged: &mut BTreeMap<PathBuf, String>,
    identity: &(PathBuf, String),
    context: &MergeContext<'_>,
) -> Result<(), MergeError> {
    let located = resolve_identity(staged, identity, context)?;
    let contents = staged.get(&identity.0).cloned().ok_or_else(|| {
        MergeError::new(
            "task_not_found",
            format!("no note matches `{}`", identity.0.display()),
        )
    })?;
    let mut lines = contents
        .split_inclusive('\n')
        .map(str::to_string)
        .collect::<Vec<_>>();
    let Some(line) = lines.get_mut(located.task.line_index) else {
        return Err(MergeError::new(
            "task_not_found",
            format!("no task `^{}` in `{}`", identity.1, identity.0.display()),
        ));
    };
    let ending = if line.ends_with("\r\n") {
        "\r\n"
    } else if line.ends_with('\n') {
        "\n"
    } else {
        ""
    };
    let body = line.strip_suffix(ending).unwrap_or(line).to_string();
    let status = successors::successor_new_status(located.task.status_symbol);
    let mut updated = set_task_line_status(&body, status).unwrap_or(body);
    updated = stamp_fresh(&updated, context.today).line;
    updated.push_str(ending);
    *line = updated;
    staged.insert(identity.0.clone(), lines.concat());
    Ok(())
}

fn build_records(
    sources: &[SourceState],
    prepared: Option<&PreparedMerge>,
    reports: &[RetirementReports],
    target: &MergeTargetDescriptor,
    label: &str,
    context: &MergeContext<'_>,
    staged: &BTreeMap<PathBuf, String>,
) -> Vec<MergeRecord> {
    sources
        .iter()
        .map(|source| {
            let identity = source_identity(&source.located);
            let prepared_source = prepared.and_then(|prepared| {
                prepared.sources.iter().find(|candidate| {
                    candidate.path == identity.path
                        && candidate.block_id == identity.block_id
                })
            });
            let report_index = prepared.and_then(|prepared| {
                prepared.sources.iter().position(|candidate| {
                    candidate.path == identity.path
                        && candidate.block_id == identity.block_id
                })
            });
            let report = report_index
                .and_then(|index| reports.get(index))
                .cloned()
                .unwrap_or_default();
            let outcome = match source.outcome {
                SourceOutcome::New => MergeOutcome::Merged,
                SourceOutcome::Unchanged => MergeOutcome::Unchanged,
                SourceOutcome::AlreadyClosed => MergeOutcome::AlreadyClosed,
            };
            let moved_lines = prepared_source
                .map(|source| source.moved_lines)
                .unwrap_or_default();
            let moved_notes = prepared_source
                .map(|source| source.moved_notes)
                .unwrap_or_default();
            let target_title = target_display(target);
            let source_title = note_tasks::clean_description(
                &source.located.task.description,
                &context.task_settings.global_filter,
                source.located.task.block_id.as_deref(),
            );
            let message = match outcome {
                MergeOutcome::Merged => {
                    let moved = if moved_notes == 0 {
                        "title only".to_string()
                    } else {
                        format!("{moved_notes} notes moved")
                    };
                    format!(
                        "⊘ {source_title}  {}:{} · {} → Cancelled · {moved}",
                        source.located.path.display(),
                        source.located.line,
                        source.located.task.status_name,
                    )
                }
                MergeOutcome::Unchanged => {
                    format!("Already merged into “{target_title}”")
                }
                MergeOutcome::AlreadyClosed => format!(
                    "Recorded merge relation to “{target_title}”; source was already closed"
                ),
            };
            let consequences = build_consequences(
                &source_title,
                &target_title,
                &report,
                staged,
                context,
            );
            let cancelled_name =
                vault_status_name(context.task_settings, '-');
            MergeRecord {
                source: MergeTaskDescriptor {
                    path: source.located.path.clone(),
                    block_id: identity.block_id,
                    line: source.located.line,
                    title: source_title,
                },
                target: target.clone(),
                label: label.to_string(),
                outcome,
                previous_status: Some(source.located.task.status_symbol),
                previous_status_name: Some(
                    source.located.task.status_name.clone(),
                ),
                status_symbol: Some('-'),
                status_name: Some(cancelled_name),
                moved_lines,
                moved_notes,
                dependents_repointed: report.dependents_repointed,
                prerequisites_left: report.prerequisites_left,
                successors_orphaned: report.successors_orphaned,
                links_pruned: report.links_pruned,
                consequences,
                offers: Vec::new(),
                message,
            }
        })
        .collect()
}

fn vault_status_name(
    settings: &crate::native::note_tasks::NoteTaskSettings,
    symbol: char,
) -> String {
    settings
        .status_definitions
        .iter()
        .find(|definition| {
            let mut characters = definition.symbol.chars();
            characters.next() == Some(symbol) && characters.next().is_none()
        })
        .map(|definition| definition.name.clone())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| {
            if symbol == '-' {
                "Cancelled".to_string()
            } else {
                "Unknown".to_string()
            }
        })
}

fn title_for_key(
    notes: &BTreeMap<PathBuf, String>,
    key: &str,
    settings: &crate::native::note_tasks::NoteTaskSettings,
) -> Option<String> {
    let (path_text, block_id) = key.split_once("#^")?;
    let path = PathBuf::from(path_text);
    let contents = notes.get(&path)?;
    let scan = note_tasks::scan(contents, settings);
    let task = scan
        .tasks()
        .iter()
        .find(|task| task.block_id.as_deref() == Some(block_id))?;
    Some(note_tasks::clean_description(
        &task.description,
        &settings.global_filter,
        task.block_id.as_deref(),
    ))
}

fn quoted_title_or_fallback(
    notes: &BTreeMap<PathBuf, String>,
    key: &str,
    settings: &crate::native::note_tasks::NoteTaskSettings,
) -> String {
    if let Some(title) = title_for_key(notes, key, settings)
        && !title.is_empty()
    {
        return format!("“{title}”");
    }
    let path = key.split_once("#^").map(|(path, _)| path).unwrap_or(key);
    format!("a task in {path}")
}

fn hint_for_key(key: &str) -> String {
    let (path_text, block_id) = key
        .split_once("#^")
        .map_or((key, ""), |(path, id)| (path, id));
    let stem = path_text.strip_suffix(".md").unwrap_or(path_text);
    if block_id.is_empty() {
        format!("&{stem}")
    } else {
        format!("&{stem}:{block_id}")
    }
}

fn build_consequences(
    source_title: &str,
    target_title: &str,
    report: &RetirementReports,
    staged: &BTreeMap<PathBuf, String>,
    context: &MergeContext<'_>,
) -> Vec<MergeConsequence> {
    let mut out = Vec::new();
    for key in &report.dependents_repointed {
        let dependent =
            quoted_title_or_fallback(staged, key, context.task_settings);
        out.push(MergeConsequence {
            kind: "dependent".to_string(),
            tone: "info".to_string(),
            text: format!("{dependent} now waits on “{target_title}”"),
            keys: vec![key.clone()],
        });
    }
    if !report.prerequisites_left.is_empty() {
        let names: Vec<String> = report
            .prerequisites_left
            .iter()
            .map(|key| {
                if let Some(title) =
                    title_for_key(staged, key, context.task_settings)
                    && !title.is_empty()
                {
                    format!("“{title}”")
                } else {
                    let path = key
                        .split_once("#^")
                        .map(|(path, _)| path)
                        .unwrap_or(key.as_str());
                    format!("a task in {path}")
                }
            })
            .collect();
        let hints: Vec<String> = report
            .prerequisites_left
            .iter()
            .map(|key| hint_for_key(key))
            .collect();
        out.push(MergeConsequence {
            kind: "prerequisite_left".to_string(),
            tone: "warning".to_string(),
            text: format!(
                "“{source_title}” waited on {}; “{target_title}” does not · add {}",
                names.join(", "),
                hints.join(", "),
            ),
            keys: report.prerequisites_left.clone(),
        });
    }
    if !report.successors_orphaned.is_empty() {
        let count = report.successors_orphaned.len();
        out.push(MergeConsequence {
            kind: "successor_orphaned".to_string(),
            tone: "warning".to_string(),
            text: format!(
                "{count} task(s) start after “{source_title}”; they won't be offered when “{target_title}” finishes · relink with ~"
            ),
            keys: report.successors_orphaned.clone(),
        });
    }
    if !report.pruned_entries.is_empty() {
        for entry in &report.pruned_entries {
            out.push(MergeConsequence {
                kind: "link_pruned".to_string(),
                tone: "info".to_string(),
                text: format!("“{source_title}” leaves {entry}"),
                keys: Vec::new(),
            });
        }
    } else if report.links_pruned > 0 {
        out.push(MergeConsequence {
            kind: "link_pruned".to_string(),
            tone: "info".to_string(),
            text: format!(
                "“{source_title}” leaves {} of today's Pomodoros",
                report.links_pruned
            ),
            keys: Vec::new(),
        });
    }
    out
}

fn target_display(target: &MergeTargetDescriptor) -> String {
    target
        .title
        .clone()
        .or_else(|| target.drawer.clone())
        .or_else(|| target.section.clone())
        .unwrap_or_else(|| target.path.to_string_lossy().into_owned())
}

fn enrich_target_lines(
    records: &mut [MergeRecord],
    staged: &BTreeMap<PathBuf, String>,
    context: &MergeContext<'_>,
) {
    for record in records.iter_mut() {
        let line = target_line(&record.target, staged, context);
        record.target.line = line;
        if (record.target.kind == "task" || record.target.kind == "drawer")
            && let Some(contents) = staged.get(&record.target.path)
        {
            let scan = note_tasks::scan(contents, context.task_settings);
            if let Some(block_id) = record.target.block_id.as_deref()
                && let Some(task) = scan
                    .tasks()
                    .iter()
                    .find(|task| task.block_id.as_deref() == Some(block_id))
            {
                record.target.status_symbol = Some(task.status_symbol);
                record.target.status_name = Some(task.status_name.clone());
            }
        }
    }
}

fn target_line(
    target: &MergeTargetDescriptor,
    staged: &BTreeMap<PathBuf, String>,
    context: &MergeContext<'_>,
) -> Option<usize> {
    let contents = staged.get(&target.path)?;
    if target.kind == "section" {
        let section = target.section.as_deref()?;
        for (index, line) in contents.lines().enumerate() {
            if let Some((level, title)) =
                crate::native::markdown::atx_heading(line)
                && level == 2
                && title == section
            {
                return Some(index + 1);
            }
        }
        return None;
    }
    let block_id = target.block_id.as_deref()?;
    let scan = note_tasks::scan(contents, context.task_settings);
    scan.tasks()
        .iter()
        .find(|task| task.block_id.as_deref() == Some(block_id))
        .map(|task| task.line_index + 1)
}

fn ordered_paths(
    postimages: &BTreeMap<PathBuf, String>,
    sources: &[SourceState],
    target_path: &Path,
    reports: &[RetirementReports],
    repaired_files: &[super::super::task_move::transplant::RepairedFile],
    daily_path: &Path,
) -> Vec<PathBuf> {
    let source_paths = sources
        .iter()
        .map(|source| source.located.path.clone())
        .collect::<BTreeSet<_>>();
    let dependent_paths = reports
        .iter()
        .flat_map(|report| report.dependents_repointed.iter())
        .filter_map(|dependent| {
            dependent
                .split_once('#')
                .map(|(path, _)| PathBuf::from(path))
        })
        .collect::<BTreeSet<_>>();
    let repaired_paths = repaired_files
        .iter()
        .map(|file| file.path.clone())
        .collect::<BTreeSet<_>>();
    let mut ranked = postimages
        .keys()
        .cloned()
        .map(|path| {
            let rank = if path == target_path {
                0
            } else if dependent_paths.contains(&path) {
                1
            } else if repaired_paths.contains(&path) {
                2
            } else if path == daily_path {
                3
            } else if source_paths.contains(&path) {
                4
            } else {
                2
            };
            (rank, path)
        })
        .collect::<Vec<_>>();
    ranked.sort();
    ranked.into_iter().map(|(_, path)| path).collect()
}

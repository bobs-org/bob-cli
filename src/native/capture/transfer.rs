//! Staged `>` transfers through the shared move and explicit merge engines.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

use chrono::NaiveDate;

use super::batch_move::{apply_batch_move, BatchMoveError};
use super::*;
use crate::native::capture_language::transfer as lexer;
use crate::native::task_move::{
    IdCollisionPolicy, MovePlacement, MoveRequest, MoveSelector, PlanError,
    VaultSnapshot,
};
use crate::native::vault_links::{target_to_markdown_path, NoteIndex};

pub(super) const TRANSFER_OVERLAPPING_SOURCES_CODE: &str =
    "transfer_overlapping_sources";
/// One moved task in a transfer item result. This mirrors the
/// `bob task move` `MoveRecord` JSON shape exactly (same keys, same
/// nulls for absent optionals) so Mac clients share one decoder.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(super) struct TransferMoveJson {
    pub(super) source_path: String,
    pub(super) destination_path: String,
    pub(super) source_line: usize,
    pub(super) destination_line: usize,
    pub(super) destination_section: Option<String>,
    pub(super) status_symbol: String,
    pub(super) status_name: String,
    pub(super) title: String,
    pub(super) old_block_id: Option<String>,
    pub(super) new_block_id: Option<String>,
    pub(super) old_dependency_id: Option<String>,
    pub(super) new_dependency_id: Option<String>,
}

/// One vault-wide repair row in a transfer item result. Same shape as
/// the `bob task move` `repaired_files[]` entries.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(super) struct TransferRepairedFileJson {
    pub(super) path: String,
    pub(super) link_count: usize,
    pub(super) dependency_token_count: usize,
}

/// Grouped transfer-move output flattened onto [`CaptureItemResult`].
/// Defaulted so ordinary items omit both keys and stay byte-identical.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub(super) struct TransferMoveOutput {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) moves: Vec<TransferMoveJson>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) repaired_files: Vec<TransferRepairedFileJson>,
}

/// Plan one atomic transfer item against the current staged vault.
pub(super) fn plan_transfer_item(
    request: &CaptureRequest,
    parsed_item: ParsedCaptureItem,
    today: NaiveDate,
    planner: &mut CaptureBatchPlanner,
    warnings: &mut Vec<CaptureWarning>,
    ctx: &DependencyContext,
) -> Result<PlannedCaptureItem, CaptureError> {
    let ParsedCaptureText { kind, route, .. } = &parsed_item.parsed;
    let (sources, destination) = match kind {
        CaptureKind::Transfer {
            sources,
            destination,
            ..
        } => (sources.clone(), destination.clone()),
        _ => {
            return Err(CaptureError::io(
                "transfer capture invariant failed: wrong capture kind",
            ));
        }
    };
    let is_move = destination.kind == lexer::TransferDestinationKind::Note;
    // Resolve every source before mutation; aliases and overlapping blocks refuse
    // as a whole. Merge snapshots also guard the scanned membership and bytes.
    let snapshot = if is_move {
        super::batch_move::destination_snapshot(&request.bob_dir, planner)?
    } else {
        super::relations_merge::load_staged_snapshot(planner, &request.bob_dir)?
    };
    let dest_path = resolve_transfer_note(&snapshot, &destination.route)
        .map_err(|error| {
            CaptureError::usage_code(error.to_string(), error.code())
        })?;
    let mut resolved_sources = Vec::with_capacity(sources.len());
    for source in &sources {
        let path = resolve_transfer_note(&snapshot, &source.route).map_err(
            |error| CaptureError::usage_code(error.to_string(), error.code()),
        )?;
        resolved_sources.push((source.clone(), path));
    }
    let settings = note_tasks::read_settings(&request.bob_dir);
    refuse_resolved_duplicates(&resolved_sources)?;
    refuse_overlapping_sources(&snapshot, &resolved_sources, &settings)?;
    // A source already moved by an earlier item in this batch gets the
    // same already-moved teaching error as project-note move bullets.
    for (source, path) in &resolved_sources {
        let absolute = request.bob_dir.join(path);
        if let Some(previous) =
            planner.moved_origin(&absolute, &source.block_id)
        {
            let destination = previous
                .destination
                .strip_prefix(&request.bob_dir)
                .unwrap_or(&previous.destination);
            return Err(CaptureError::usage_code(
                format!(
                    "`^{}` already moved to {} in item {}",
                    source.block_id,
                    super::dependencies::display_relative(destination),
                    previous.item_number
                ),
                "move_task_already_moved",
            ));
        }
    }
    let dest_display = super::dependencies::display_relative(&dest_path);
    let dest_absolute = request.bob_dir.join(&dest_path);
    let item_number = parsed_item.index + 1;
    let mut relation_merge_output = RelationMergeOutput::default();
    let mut moves = Vec::new();
    let mut repaired: BTreeMap<PathBuf, (usize, usize)> = BTreeMap::new();
    let mut needs_reconcile = false;
    if is_move {
        for (source, path) in &resolved_sources {
            let source_display = super::dependencies::display_relative(path);
            let move_request = MoveRequest {
                source: source_display,
                dest: dest_display.clone(),
                selectors: vec![MoveSelector::Block(source.block_id.clone())],
                allow_closed: false,
                placement: MovePlacement::Append,
                id_collision: IdCollisionPolicy::Refuse,
            };
            let plan = match apply_batch_move(
                planner,
                &request.bob_dir,
                &settings,
                today,
                &move_request,
            ) {
                Ok(plan) => plan,
                Err(BatchMoveError::Plan(error)) => {
                    return Err(CaptureError::usage_code(
                        error.to_string(),
                        error.code(),
                    ));
                }
                Err(BatchMoveError::Capture(error)) => return Err(error),
            };
            for movement in &plan.moves {
                moves.push(TransferMoveJson {
                    source_path: super::dependencies::display_relative(
                        &movement.source_path,
                    ),
                    destination_path: super::dependencies::display_relative(
                        &movement.dest_path,
                    ),
                    source_line: movement.source_line,
                    destination_line: movement.dest_line,
                    destination_section: movement.destination_section.clone(),
                    status_symbol: movement.status_symbol.to_string(),
                    status_name: super::task_complete::status_name_for(
                        &request.bob_dir,
                        movement.status_symbol,
                    ),
                    title: movement.title.clone(),
                    old_block_id: movement.old_block_id.clone(),
                    new_block_id: movement.new_block_id.clone(),
                    old_dependency_id: movement.old_dependency_id.clone(),
                    new_dependency_id: movement.new_dependency_id.clone(),
                });
            }
            for file in &plan.repaired_files {
                let entry = repaired.entry(file.path.clone()).or_insert((0, 0));
                entry.0 += file.link_count;
                entry.1 += file.dependency_token_count;
            }
            needs_reconcile |= plan.needs_reconcile;
            // Later items see the new identity through the staged overlay;
            // a repeat of the old identity gets the already-moved error
            // above instead of the engine's missing-selector refusal.
            for movement in &plan.moves {
                if let Some(old) = movement.old_block_id.as_deref() {
                    planner.remap_created_task(
                        &request.bob_dir.join(&movement.source_path),
                        old,
                        &request.bob_dir.join(&movement.dest_path),
                        movement.new_block_id.as_deref(),
                    );
                    planner.record_moved_origin(
                        &request.bob_dir.join(&movement.source_path),
                        old,
                        &request.bob_dir.join(&movement.dest_path),
                        item_number,
                    );
                }
            }
        }
    } else {
        use crate::native::{
            task_locator::TaskLocator, task_merge::MergeContainer,
        };

        let locator = |path: &Path, id: &str| TaskLocator {
            note: path.with_extension("").to_string_lossy().replace('\\', "/"),
            block_id: id.to_string(),
        };
        // The transfer adapter owns `>` destination parsing and its special
        // refusal. The explicit engine below only receives a resolved
        // container, which lets `<` share the same merge behavior later.
        let target = match destination.kind {
            lexer::TransferDestinationKind::Task
            | lexer::TransferDestinationKind::Drawer => {
                let id = destination
                    .block_id
                    .as_deref()
                    .expect("complete task destination");
                if resolved_sources.iter().any(|(source, path)| {
                    path == &dest_path && source.block_id == id
                }) {
                    return Err(CaptureError::usage_code(
                        "a transfer destination cannot also be a source",
                        "transfer_destination_is_source",
                    ));
                }
                let task = locator(&dest_path, id);
                if let Some(drawer) = &destination.drawer {
                    MergeContainer::Drawer(
                        task,
                        capture_language::DrawerSelector {
                            text: drawer.clone(),
                            exact: destination.drawer_exact,
                        },
                    )
                } else {
                    MergeContainer::Task(task)
                }
            }
            lexer::TransferDestinationKind::Section => {
                MergeContainer::Section {
                    note: dest_path
                        .with_extension("")
                        .to_string_lossy()
                        .replace('\\', "/"),
                    selector: capture_language::DrawerSelector {
                        text: destination
                            .section
                            .clone()
                            .expect("complete section destination"),
                        exact: destination.section_exact,
                    },
                }
            }
            lexer::TransferDestinationKind::Note => {
                unreachable!("moves dispatch separately")
            }
        };
        let (output, repairs) = plan_explicit_merge(
            request,
            planner,
            ctx,
            &snapshot,
            &resolved_sources,
            target,
            warnings,
        )?;
        relation_merge_output = output;
        for file in repairs {
            repaired.insert(
                file.path,
                (file.link_count, file.dependency_token_count),
            );
        }
    }
    let repaired_files = repaired
        .into_iter()
        .map(|(path, (link_count, dependency_token_count))| {
            TransferRepairedFileJson {
                path: super::dependencies::display_relative(&path),
                link_count,
                dependency_token_count,
            }
        })
        .collect::<Vec<_>>();
    if needs_reconcile {
        warnings.push(CaptureWarning::Text(
            "Project blocking changed; run bob task reconcile.".to_string(),
        ));
    }
    let task_block_refs = transfer_task_block_refs(
        planner,
        &request.bob_dir,
        &settings,
        is_move,
        &moves,
        &relation_merge_output,
    );
    let created = super::plan::date_string(today);
    let kind_label = super::plan::capture_kind_label(&parsed_item.parsed.kind);
    let route_label = dest_display.clone();
    let route_stem = dest_path
        .with_extension("")
        .to_string_lossy()
        .replace('\\', "/");
    // Keep the lexical destination route when it already names the
    // resolved note (the common root-note case); otherwise report the
    // resolved stem so subdir destinations stay truthful.
    let result_route = if destination.route == route_stem {
        destination.route.clone()
    } else if route.as_deref() == Some(route_stem.as_str()) {
        route.clone().unwrap_or(route_stem.clone())
    } else {
        route_stem.clone()
    };
    Ok(PlannedCaptureItem {
        result: CaptureItemResult {
            note_pull: None,
            task_theme: None,
            type_relocation: None,
            ok: true,
            dry_run: request.dry_run,
            routed: true,
            route: Some(result_route),
            route_label,
            relative_target: dest_display,
            target: dest_absolute.display().to_string(),
            text: String::new(),
            task_line: String::new(),
            kind: kind_label,
            fanout: None,
            created,
            scheduled: None,
            priority: None,
            priority_label: None,
            placement: Placement::Updated,
            sub_bullets: Vec::new(),
            notes: Vec::new(),
            drawers: Vec::new(),
            clip: None,
            schedule_log: None,
            block_id: None,
            day_file: None,
            block_link: None,
            pomodoro_link_placement: None,
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
            pomodoro_split: None,
            pomodoro_merge: None,
            pomodoro_close: None,
            pomodoro_past: None,
            pomodoro_reset: None,
            dependency_update: None,
            consumed_links: Vec::new(),
            task_complete: None,
            toggle_task_description: None,
            r#ref: None,
            gmail: Vec::new(),
            link_guard: None,
            dropped_dependencies: Vec::new(),
            blocks_line_updates: Vec::new(),
            minted_dependent_block_id: None,
            dependency_updates: Vec::new(),
            minted_blocker_block_id: None,
            relation_updates: Vec::new(),
            relation_cancels: Vec::new(),
            relation_completes: Vec::new(),
            relation_merge_output,
            transfer_move_output: TransferMoveOutput {
                moves,
                repaired_files,
            },
            relation_label_hides: Vec::new(),
            linked_from_line_updates: Vec::new(),
            minted_relation_block_ids: Vec::new(),
            mirror_skipped: Vec::new(),
            list: None,
            relation_successors: Vec::new(),
            relation_held: Vec::new(),
            relation_check: None,
            relation_groups: Vec::new(),
        },
        clip_plan: None,
        pomodoro_refs: Vec::new(),
        task_block_refs,
        project_move_landings: Vec::new(),
    })
}

/// Build final-state tracker refs for the blocks a transfer moved or merged.
/// Each staged note is scanned at most once for this item, no matter how many
/// sources or destinations it contributes.
fn transfer_task_block_refs(
    planner: &CaptureBatchPlanner,
    bob_dir: &Path,
    settings: &note_tasks::NoteTaskSettings,
    is_move: bool,
    moves: &[TransferMoveJson],
    merge_output: &RelationMergeOutput,
) -> Vec<TaskBlockRef> {
    if !is_move {
        return merge_task_block_refs(planner, bob_dir, settings, merge_output);
    }
    let mut scans: HashMap<PathBuf, note_tasks::NoteTaskScan> = HashMap::new();
    let mut refs = Vec::new();
    for movement in moves {
        let Some(block_id) = movement.new_block_id.as_deref() else {
            continue;
        };
        let path = PathBuf::from(&movement.destination_path);
        let target = bob_dir.join(&path);
        let Some(text) = planner.peek_text(&target) else {
            continue;
        };
        let scan = scans
            .entry(target.clone())
            .or_insert_with(|| note_tasks::scan(&text, settings));
        let BlockIdLookup::Found(task) = scan.by_block_id(block_id) else {
            debug_assert!(
                false,
                "moved task is missing from staged destination"
            );
            continue;
        };
        refs.push(TaskBlockRef {
            target,
            relative_target: movement.destination_path.clone(),
            route: transfer_route(&path),
            line: task.line_index,
            block_id: Some(block_id.to_string()),
            role: TaskBlockRole::Moved,
        });
    }
    refs
}

/// Build final-state tracker refs for a task merge, usable by both capture
/// `>` and capture `<` adapters.
pub(super) fn merge_task_block_refs(
    planner: &CaptureBatchPlanner,
    bob_dir: &Path,
    settings: &note_tasks::NoteTaskSettings,
    merge_output: &RelationMergeOutput,
) -> Vec<TaskBlockRef> {
    use crate::native::task_merge::MergeOutcome;

    let mut scans: HashMap<PathBuf, note_tasks::NoteTaskScan> = HashMap::new();
    let mut refs = Vec::new();

    let merged = merge_output
        .merges
        .iter()
        .filter(|record| record.outcome == MergeOutcome::Merged)
        .collect::<Vec<_>>();
    let Some(first) = merged.first() else {
        return refs;
    };
    if first.target.kind != "section"
        && let Some(block_id) = first.target.block_id.as_deref()
    {
        let path = first.target.path.clone();
        let target = bob_dir.join(&path);
        if let Some(text) = planner.peek_text(&target) {
            let scan = scans
                .entry(target.clone())
                .or_insert_with(|| note_tasks::scan(&text, settings));
            if let BlockIdLookup::Found(task) = scan.by_block_id(block_id) {
                refs.push(TaskBlockRef {
                    target,
                    relative_target: super::dependencies::display_relative(
                        &path,
                    ),
                    route: transfer_route(&path),
                    line: task.line_index,
                    block_id: Some(block_id.to_string()),
                    role: TaskBlockRole::MergeTarget,
                });
            } else {
                debug_assert!(
                    false,
                    "merge target is missing from staged text"
                );
            }
        }
    }
    for record in merged {
        let path = record.source.path.clone();
        let target = bob_dir.join(&path);
        let Some(text) = planner.peek_text(&target) else {
            continue;
        };
        let scan = scans
            .entry(target.clone())
            .or_insert_with(|| note_tasks::scan(&text, settings));
        let BlockIdLookup::Found(task) =
            scan.by_block_id(&record.source.block_id)
        else {
            debug_assert!(false, "merge tombstone is missing from staged text");
            continue;
        };
        refs.push(TaskBlockRef {
            target,
            relative_target: super::dependencies::display_relative(&path),
            route: transfer_route(&path),
            line: task.line_index,
            block_id: Some(record.source.block_id.clone()),
            role: TaskBlockRole::MergeSource,
        });
    }
    refs
}

fn transfer_route(path: &Path) -> String {
    path.with_extension("").to_string_lossy().replace('\\', "/")
}

/// Mark task targets born in this capture draft. Drawer and section creation
/// keeps the engine's own `created` meaning and is additive to task birth.
pub(super) fn mark_created_merge_targets(
    planner: &CaptureBatchPlanner,
    bob_dir: &Path,
    records: &mut [crate::native::task_merge::MergeRecord],
) {
    for record in records {
        let Some(block_id) = record.target.block_id.as_deref() else {
            continue;
        };
        if planner
            .task_was_created(&bob_dir.join(&record.target.path), block_id)
        {
            record.target.created = true;
        }
    }
}

/// Resolve one transfer route against the staged snapshot's vault
/// index. This adapts route resolution instead of assuming routes
/// equal filesystem paths: basename stems match case-insensitively
/// across subdirectories, mirroring `plan_task_move`.
pub(super) fn resolve_transfer_note(
    snapshot: &VaultSnapshot,
    query: &str,
) -> Result<PathBuf, PlanError> {
    use std::path::Component;
    if query.trim().is_empty()
        || target_to_markdown_path(query).is_none()
        || Path::new(query)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(PlanError::InvalidPath {
            query: query.to_string(),
        });
    }
    let index = NoteIndex::from_paths(snapshot.notes.keys().cloned());
    if query.contains('/')
        || Path::new(query)
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
    {
        let Some(candidate) = target_to_markdown_path(query) else {
            return Err(PlanError::InvalidPath {
                query: query.to_string(),
            });
        };
        let mut exact: Vec<PathBuf> = index
            .relative_paths()
            .filter(|path| *path == candidate)
            .map(Path::to_path_buf)
            .collect();
        if exact.is_empty() {
            exact = index
                .relative_paths()
                .filter(|path| {
                    path.as_os_str().to_string_lossy().eq_ignore_ascii_case(
                        &candidate.as_os_str().to_string_lossy(),
                    )
                })
                .map(Path::to_path_buf)
                .collect();
        }
        return match exact.as_slice() {
            [path] => Ok(path.clone()),
            [] => Err(PlanError::MissingNote {
                query: query.to_string(),
            }),
            _ => Err(PlanError::AmbiguousNote {
                query: query.to_string(),
                candidates: exact,
            }),
        };
    }
    let mut candidates = index.basename_candidates(query);
    candidates.sort();
    candidates.dedup();
    match candidates.as_slice() {
        [path] => Ok(path.clone()),
        [] => Err(PlanError::MissingNote {
            query: query.to_string(),
        }),
        _ => Err(PlanError::AmbiguousNote {
            query: query.to_string(),
            candidates,
        }),
    }
}

/// Refuse resolved duplicate source identities, catching aliases the
/// lexical lower-cased route check cannot see.
pub(super) fn refuse_resolved_duplicates(
    resolved: &[(lexer::TransferSource, PathBuf)],
) -> Result<(), CaptureError> {
    let mut seen: HashMap<(PathBuf, String), &lexer::TransferSource> =
        HashMap::new();
    for (source, path) in resolved {
        let key = (path.clone(), source.block_id.clone());
        if let Some(_first) = seen.get(&key) {
            return Err(CaptureError::usage_code(
                crate::native::capture_language::target_runs::duplicate_target_message(
                    &source.marker,
                    &source.block_id,
                ),
                lexer::TRANSFER_DUPLICATE_CODE,
            ));
        }
        seen.insert(key, source);
    }
    Ok(())
}

/// Refuse a source nested inside another source's task block before
/// any source stages. Column-zero siblings never overlap; a hit means
/// one source names a parent and another names its descendant.
pub(super) fn refuse_overlapping_sources(
    snapshot: &VaultSnapshot,
    resolved: &[(lexer::TransferSource, PathBuf)],
    settings: &note_tasks::NoteTaskSettings,
) -> Result<(), CaptureError> {
    let mut by_path: BTreeMap<
        &PathBuf,
        Vec<&(lexer::TransferSource, PathBuf)>,
    > = BTreeMap::new();
    for entry in resolved {
        by_path.entry(&entry.1).or_default().push(entry);
    }
    for (path, entries) in by_path {
        if entries.len() < 2 {
            continue;
        }
        let Some(contents) = snapshot.notes.get(path) else {
            continue;
        };
        let scan = note_tasks::scan(contents, settings);
        let inclusive: Vec<&str> = contents.split_inclusive('\n').collect();
        let mut ranges: Vec<(&lexer::TransferSource, usize, usize)> =
            Vec::new();
        for (source, _) in entries {
            let line = match scan.by_block_id(&source.block_id) {
                note_tasks::BlockIdLookup::Found(task) => task.line_index,
                _ => continue,
            };
            let end = crate::native::collect_done::task_block_end(
                &inclusive, line, 0,
            );
            ranges.push((source, line, end.max(line + 1)));
        }
        for i in 0..ranges.len() {
            for j in 0..ranges.len() {
                if i == j {
                    continue;
                }
                let (outer, outer_start, outer_end) = ranges[i];
                let (inner, inner_start, _) = ranges[j];
                if inner_start >= outer_start && inner_start < outer_end {
                    // Skip the exact-duplicate case; the duplicate
                    // check above already owns it.
                    if outer.block_id == inner.block_id {
                        continue;
                    }
                    return Err(CaptureError::usage_code(
                        format!(
                            "`{}` is nested under `{}`; move the parent to bring the whole block",
                            inner.marker, outer.marker
                        ),
                        TRANSFER_OVERLAPPING_SOURCES_CODE,
                    ));
                }
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn plan_explicit_merge(
    request: &CaptureRequest,
    planner: &mut CaptureBatchPlanner,
    ctx: &DependencyContext,
    snapshot: &VaultSnapshot,
    sources: &[(lexer::TransferSource, PathBuf)],
    target: crate::native::task_merge::MergeContainer,
    warnings: &mut Vec<CaptureWarning>,
) -> Result<
    (
        RelationMergeOutput,
        Vec<crate::native::task_move::transplant::RepairedFile>,
    ),
    CaptureError,
> {
    use crate::native::{
        task_locator::TaskLocator,
        task_merge::{
            plan_task_merge_with_selected_freshness, DailyNoteReadState,
            MergeContext, MergeGesture, MergeRelationPolicy, MergeRequest,
        },
    };
    let locator = |path: &Path, id: &str| TaskLocator {
        note: path.with_extension("").to_string_lossy().replace('\\', "/"),
        block_id: id.to_string(),
    };
    let daily_abs = pomodoro::day_file_for(&request.bob_dir);
    let daily_path = daily_abs
        .strip_prefix(&request.bob_dir)
        .map_err(|error| CaptureError::io(error.to_string()))?
        .to_path_buf();
    let note_index = NoteIndex::from_paths(snapshot.notes.keys().cloned());
    let drawers = capture_drawers::load_drawer_catalog(&request.bob_dir)
        .unwrap_or_default();
    let context = MergeContext {
        today: ctx.today,
        daily_read: snapshot
            .notes
            .get(&daily_path)
            .cloned()
            .map(DailyNoteReadState::Read)
            .unwrap_or(DailyNoteReadState::Missing),
        daily_path: daily_path.clone(),
        task_settings: ctx.settings(),
        note_index: &note_index,
        drawer_catalog: &drawers,
        relation_policy: MergeRelationPolicy {
            linked_from_lines: ctx.linked_from_lines_enabled(),
            linked_from_mentions: ctx.linked_from_mentions_enabled(),
        },
        successor_labels: ctx.successor_labels(),
    };
    let merge_request = MergeRequest {
        sources: sources
            .iter()
            .map(|(source, path)| locator(path, &source.block_id))
            .collect(),
        target,
        label: ctx
            .merge_labels()
            .first()
            .map(|label| label.name.clone())
            .unwrap_or_else(|| "merged into".to_string()),
        gesture: MergeGesture::Explicit,
        // Capture maps draft-global numbers to stable keys before this pass.
        // The engine alone applies the selected target once at its lost-link slot.
        selection: request.selection.clone(),
    };
    let stamp_selected_target_fresh = match &merge_request.target {
        crate::native::task_merge::MergeContainer::Task(locator)
        | crate::native::task_merge::MergeContainer::Drawer(locator, _) => {
            let target_path =
                request.bob_dir.join(format!("{}.md", locator.note));
            !planner.task_was_created(&target_path, &locator.block_id)
        }
        crate::native::task_merge::MergeContainer::Section { .. } => true,
    };
    let mut plan = plan_task_merge_with_selected_freshness(
        snapshot,
        &merge_request,
        &context,
        stamp_selected_target_fresh,
    )
    .map_err(|error| CaptureError::usage_code(error.message, error.code))?;
    mark_created_merge_targets(planner, &request.bob_dir, &mut plan.records);
    super::relations_merge::stage_merge_plan(
        planner,
        &request.bob_dir,
        snapshot,
        &plan,
    )?;
    let mut after = snapshot.clone();
    after.notes.extend(plan.postimages.clone());
    let offer_rows = plan
        .records
        .iter()
        .flat_map(|record| {
            super::relations_merge::offer_rows_for_record(
                record,
                snapshot,
                &after,
                ctx.settings(),
                &daily_path,
            )
        })
        .collect();
    warnings.extend(plan.warnings.into_iter().map(CaptureWarning::Text));
    // Section destinations render as section_blocks, which carry no line
    // origins. Task and drawer targets render as task_blocks with origins.
    let merge_line_origins = match &merge_request.target {
        crate::native::task_merge::MergeContainer::Section { .. } => Vec::new(),
        _ => plan.merge_line_origins,
    };
    Ok((
        RelationMergeOutput {
            merges: plan.records,
            offer_rows,
            merge_line_origins,
        },
        plan.repaired_files,
    ))
}

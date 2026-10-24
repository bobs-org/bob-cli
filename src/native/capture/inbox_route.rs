//! Batch-level detection of inbox tasks whose status changed in a capture,
//! plus the route-apply stage that moves answered candidates through the
//! task-move engine inside the same batch.
use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Component,
};

use crate::native::{
    freshness::placement::stamp_fresh,
    task_complete::is_inbox_note,
    task_move::{
        list_task_move_destinations, plan_task_move, MoveRequest, MoveSelector,
        PlanError, TaskMoveDestination, VaultSnapshot,
    },
    vault_links::{target_to_markdown_path, NoteIndex},
};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct InboxRouteReport {
    pub(super) supported: bool,
    pub(super) candidate_count: usize,
    pub(super) routed_count: usize,
    pub(super) kept_count: usize,
    pub(super) candidates: Vec<InboxRouteCandidate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) destinations: Option<Vec<TaskMoveDestination>>,
    /// Vault-wide link repair once a move is applied; absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) repaired_files: Option<Vec<InboxRouteRepairedFile>>,
    /// Block-ID renames from collision repair; absent unless a move ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) renamed_block_ids: Option<Vec<InboxRouteRename>>,
    /// Whether the batch must prompt `bob task reconcile`; present only
    /// once a move is applied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) needs_reconcile: Option<bool>,
}

impl InboxRouteReport {
    pub(super) fn empty() -> Self {
        Self {
            supported: true,
            candidate_count: 0,
            routed_count: 0,
            kept_count: 0,
            candidates: Vec::new(),
            destinations: None,
            repaired_files: None,
            renamed_block_ids: None,
            needs_reconcile: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct InboxRouteRepairedFile {
    pub(super) path: String,
    pub(super) link_count: usize,
    pub(super) dependency_token_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct InboxRouteRename {
    pub(super) path: String,
    pub(super) old_block_id: String,
    pub(super) new_block_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct InboxRouteCandidate {
    pub(super) key: String,
    pub(super) n: usize,
    pub(super) note_path: String,
    pub(super) inbox: String,
    pub(super) block_id: String,
    pub(super) line: usize,
    pub(super) text: String,
    pub(super) previous_status_symbol: char,
    pub(super) previous_status_name: String,
    pub(super) status_symbol: char,
    pub(super) status_name: String,
    pub(super) answer: Option<InboxRouteAnswer>,
    pub(super) outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) moved: Option<serde_json::Value>,
    /// The candidate root line's exact pre-move text. Never serialized:
    /// relocation identity for id-less candidates when an earlier route
    /// group already rewrote the source note.
    #[serde(skip_serializing)]
    pub(super) root_line: String,
    /// This candidate's move-group repair totals, for the human row.
    /// Never serialized: the JSON `moved` object keeps its contract
    /// shape and batch totals live in `repaired_files`.
    #[serde(skip_serializing)]
    pub(super) repaired: Option<(usize, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct InboxRouteAnswer {
    pub(super) action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) destination: Option<String>,
}

/// One routed task's landing, used to forward block trackers and
/// `open_notes` entries from the source note to the moved line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RoutedTaskMove {
    /// The candidate's index in the report. Coordinates shift when an
    /// earlier group rewrites the source note, so the report marks
    /// moved candidates by index, never by line.
    pub(super) candidate: usize,
    pub(super) source: PathBuf,
    pub(super) source_relative: String,
    pub(super) old_block_id: Option<String>,
    pub(super) old_line: usize,
    pub(super) dest: PathBuf,
    pub(super) dest_relative: String,
    pub(super) dest_route: String,
    pub(super) new_block_id: Option<String>,
    pub(super) new_line: usize,
    /// The move group's vault-wide repair totals, for the human row.
    pub(super) repaired_links: usize,
    pub(super) repaired_dep_tokens: usize,
}

#[derive(Debug)]
struct CandidateDraft {
    relative_path: PathBuf,
    original_line: usize,
    original: note_tasks::NoteTask,
    current: note_tasks::NoteTask,
}

/// Detect candidates after all capture items and the successor net pass have
/// updated the staged planner. Destination discovery is deferred until at
/// least one candidate exists.
pub(super) fn detect_inbox_routes(
    planner: &CaptureBatchPlanner,
    bob_dir: &Path,
    settings: &note_tasks::NoteTaskSettings,
    dependency_ctx: &DependencyContext,
) -> Result<InboxRouteReport, CaptureError> {
    let mut drafts = Vec::new();
    let mut original_contents = BTreeMap::new();
    let mut current_contents = BTreeMap::new();

    for file in &planner.files {
        if !file.existed || !file.present || file.original == file.current {
            continue;
        }
        let Ok(relative_path) = file.path.strip_prefix(bob_dir) else {
            continue;
        };
        let original_scan = note_tasks::scan(&file.original, settings);
        let current_scan = note_tasks::scan(&file.current, settings);
        if original_scan.tasks().is_empty() {
            continue;
        }
        let pairs = pair_task_lines(
            &file.original,
            &file.current,
            &original_scan,
            &current_scan,
        );
        let original_lines: Vec<&str> = file.original.lines().collect();
        let mut inbox_candidates = BTreeMap::new();

        for (&old_line, &new_line) in &pairs {
            let Some(old_task) = original_scan.task_at(old_line) else {
                continue;
            };
            let Some(new_task) = current_scan.task_at(new_line) else {
                continue;
            };
            if old_task.status_symbol == new_task.status_symbol {
                continue;
            }

            let candidate_line = if old_task.indentation.is_empty() {
                old_task.line_index
            } else {
                match column_zero_task_ancestor(
                    &original_lines,
                    &original_scan,
                    old_task.line_index,
                ) {
                    Some(ancestor) => ancestor,
                    None => continue,
                }
            };
            let Some(old_candidate) = original_scan.task_at(candidate_line)
            else {
                continue;
            };
            if old_candidate.block_id.as_deref() == Some("prj") {
                continue;
            }
            let Some(new_candidate_line) = pairs.get(&candidate_line).copied()
            else {
                continue;
            };
            let Some(new_candidate) = current_scan.task_at(new_candidate_line)
            else {
                continue;
            };
            if old_candidate.block_id.as_deref() == Some("prj") {
                continue;
            }
            // The root can be triggered by its own transition or by one of
            // its nested tasks. De-duplicate once per original block.
            inbox_candidates.insert(
                old_candidate.line_index,
                (old_candidate.clone(), new_candidate.clone()),
            );
        }

        for (original_line, (original, current)) in inbox_candidates {
            original_contents
                .entry(relative_path.to_path_buf())
                .or_insert_with(|| file.original.clone());
            current_contents
                .entry(relative_path.to_path_buf())
                .or_insert_with(|| file.current.clone());
            drafts.push(CandidateDraft {
                relative_path: relative_path.to_path_buf(),
                original_line,
                original,
                current,
            });
        }
    }

    drafts.sort_by(|left, right| {
        left.relative_path
            .cmp(&right.relative_path)
            .then(left.original_line.cmp(&right.original_line))
    });

    if drafts.is_empty() {
        return Ok(InboxRouteReport::empty());
    }
    let note_index = super::dependencies::staged_note_index(
        dependency_ctx,
        planner,
        bob_dir,
    );
    drafts.retain(|draft| {
        original_contents
            .get(&draft.relative_path)
            .is_some_and(|contents| {
                is_inbox_note(&draft.relative_path, contents, &note_index)
            })
    });
    if drafts.is_empty() {
        return Ok(InboxRouteReport::empty());
    }

    let mut digest_ordinals =
        std::collections::BTreeMap::<(PathBuf, String), usize>::new();
    let mut candidates = Vec::with_capacity(drafts.len());
    for (index, draft) in drafts.into_iter().enumerate() {
        let digest_ordinal = if draft.original.block_id.is_some() {
            1
        } else {
            let digest = hex::encode(Sha256::digest(
                draft.original.description.as_bytes(),
            ));
            let key = (draft.relative_path.clone(), digest[..8].to_string());
            let ordinal = digest_ordinals.entry(key).or_default();
            *ordinal += 1;
            *ordinal
        };
        let key = note_tasks::stable_task_key(
            &draft.relative_path,
            draft.original.block_id.as_deref(),
            &draft.original.description,
            settings.global_filter.as_str(),
            digest_ordinal,
        );
        let note_path = display_relative(&draft.relative_path);
        let inbox = draft
            .relative_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_string();
        // The exact pre-move root line: relocation identity for id-less
        // candidates when an earlier route group rewrote the source.
        // `line` above is 1-based in the same pre-move staged text.
        let root_line = current_contents
            .get(&draft.relative_path)
            .and_then(|contents| contents.lines().nth(draft.current.line_index))
            .unwrap_or_default()
            .to_string();
        candidates.push(InboxRouteCandidate {
            key,
            n: index + 1,
            note_path,
            inbox,
            block_id: draft.original.block_id.unwrap_or_default(),
            line: draft.current.line_index + 1,
            text: draft.original.description,
            previous_status_symbol: draft.original.status_symbol,
            previous_status_name: draft.original.status_name,
            status_symbol: draft.current.status_symbol,
            status_name: draft.current.status_name,
            answer: None,
            outcome: "pending",
            moved: None,
            root_line,
            repaired: None,
        });
    }

    let destinations = if candidates.is_empty() {
        None
    } else {
        let snapshot = destination_snapshot(bob_dir, planner)?;
        Some(list_task_move_destinations(&snapshot))
    };
    Ok(InboxRouteReport {
        supported: true,
        candidate_count: candidates.len(),
        routed_count: 0,
        kept_count: 0,
        candidates,
        destinations,
        repaired_files: None,
        renamed_block_ids: None,
        needs_reconcile: None,
    })
}

/// Answered route moves after candidate detection.
///
/// Groups move answers by (source, destination), plans one task-move per
/// group against a snapshot overlaid with the batch's staged texts, stages
/// every postimage through the planner (so the commit's preimage guard
/// covers the extra files and the whole batch rolls back on a concurrent
/// edit), stamps each moved root line, and records landings for the block
/// trackers and `open_notes`. Keeps only mark the report. Unanswered
/// candidates stay pending. Nothing is written here; the caller commits.
pub(super) struct AppliedInboxRoutes {
    pub(super) report: InboxRouteReport,
    pub(super) moves: Vec<RoutedTaskMove>,
}

pub(super) fn apply_inbox_routes(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    settings: &note_tasks::NoteTaskSettings,
    today: NaiveDate,
    mut report: InboxRouteReport,
    picks: &[InboxRoutePick],
) -> Result<AppliedInboxRoutes, CaptureError> {
    if picks.is_empty() {
        return Ok(AppliedInboxRoutes {
            report,
            moves: Vec::new(),
        });
    }
    // Resolve every answer against the planned candidates. Numbers are
    // 1-based positions in this plan; keys are opaque client strings.
    let mut answered: BTreeMap<usize, Option<String>> = BTreeMap::new();
    let mut bad = Vec::new();
    let mut keep_all = false;
    for pick in picks {
        match pick {
            InboxRoutePick::KeepAll => keep_all = true,
            InboxRoutePick::Answer {
                selector,
                destination,
            } => {
                let Some(index) =
                    resolve_route_selector(&report.candidates, selector)
                else {
                    bad.push(selector.clone());
                    continue;
                };
                match answered.get(&index) {
                    Some(existing) if *existing != *destination => {
                        let candidate = &report.candidates[index];
                        return Err(CaptureError::usage(format!(
                            "conflicting --inbox-route answers for candidate {} ({}); pass one answer per candidate",
                            candidate.n, candidate.key
                        )));
                    }
                    _ => {
                        answered.insert(index, destination.clone());
                    }
                }
            }
        }
    }
    if !bad.is_empty() {
        return Err(CaptureError::inbox_route_stale(&report, &bad));
    }
    // Resolve every move destination before planning anything, so a
    // refused answer fails before the planner changes.
    let mut move_dest: BTreeMap<usize, PathBuf> = BTreeMap::new();
    for (index, destination) in &answered {
        let Some(query) = destination.as_deref() else {
            continue;
        };
        let snapshot = destination_snapshot(bob_dir, planner)?;
        let note_index = NoteIndex::from_paths(snapshot.notes.keys().cloned());
        match resolve_route_destination(
            &snapshot,
            &note_index,
            report.destinations.as_deref().unwrap_or_default(),
            query,
        ) {
            Ok(path) => {
                move_dest.insert(*index, path);
            }
            Err(message) => {
                let candidate = &report.candidates[*index];
                // Echo the destination in its canonical note form so the
                // card can match it against the destination list.
                let shown = target_to_markdown_path(query)
                    .map(|path| display_relative(&path))
                    .unwrap_or_else(|| (*query).to_string());
                return Err(CaptureError::inbox_route_refused(
                    &report, candidate, shown, message,
                ));
            }
        }
    }
    // Group moves by (source, destination) in candidate order. Each
    // group plans against a snapshot rebuilt from the current staged
    // texts, so an earlier group that rewrote the source is visible.
    let mut groups: Vec<(PathBuf, PathBuf, Vec<usize>)> = Vec::new();
    for (index, dest) in &move_dest {
        let source = PathBuf::from(&report.candidates[*index].note_path);
        match groups.iter_mut().find(|(group_source, group_dest, _)| {
            *group_source == source && *group_dest == *dest
        }) {
            Some((_, _, members)) => members.push(*index),
            None => groups.push((source, dest.clone(), vec![*index])),
        }
    }
    let mut repaired: BTreeMap<PathBuf, (usize, usize)> = BTreeMap::new();
    let mut renamed = Vec::new();
    let mut needs_reconcile = false;
    let mut moves = Vec::new();
    for (source, dest, members) in &groups {
        let snapshot = destination_snapshot(bob_dir, planner)?;
        let source_text = snapshot.notes.get(source).cloned();
        let mut selectors = Vec::with_capacity(members.len());
        for index in members {
            let candidate = &report.candidates[*index];
            if candidate.block_id.is_empty() {
                let Some(source_text) = source_text.as_deref() else {
                    return Err(CaptureError::io(format!(
                        "routing lost its source note {}",
                        source.display()
                    )));
                };
                let Some(line) =
                    relocate_source_line(source_text, settings, candidate)
                else {
                    return Err(CaptureError::io(format!(
                        "routing lost candidate {} ({}) in {}",
                        candidate.n,
                        candidate.key,
                        source.display()
                    )));
                };
                selectors.push(MoveSelector::Line(line));
            } else {
                selectors.push(MoveSelector::Block(candidate.block_id.clone()));
            }
        }
        let request = MoveRequest {
            source: display_relative(source),
            dest: display_relative(dest),
            selectors,
            allow_closed: true,
        };
        let plan = match plan_task_move(&snapshot, &request) {
            Ok(plan) => plan,
            Err(error) => {
                return Err(route_engine_refusal(
                    &report, members, &request, error,
                ));
            }
        };
        for (path, link_count, dependency_token_count) in
            plan.repaired_files.iter().map(|repaired| {
                (
                    repaired.path.clone(),
                    repaired.link_count,
                    repaired.dependency_token_count,
                )
            })
        {
            let entry = repaired.entry(path).or_insert((0, 0));
            entry.0 += link_count;
            entry.1 += dependency_token_count;
        }
        for rename in &plan.renamed_block_ids {
            renamed.push(InboxRouteRename {
                path: display_relative(&rename.path),
                old_block_id: rename.old_block_id.clone(),
                new_block_id: rename.new_block_id.clone(),
            });
        }
        needs_reconcile |= plan.needs_reconcile;
        let group_links: usize =
            plan.repaired_files.iter().map(|file| file.link_count).sum();
        let group_deps: usize = plan
            .repaired_files
            .iter()
            .map(|file| file.dependency_token_count)
            .sum();
        for (path, postimage) in &plan.postimages {
            let absolute = bob_dir.join(path);
            planner.ensure_loaded(&absolute)?;
            planner.stage(&absolute, postimage.clone())?;
        }
        // Stamp each moved root line in the staged destination text and
        // record its final landing. The stamp is an in-place line edit,
        // so later groups appending to the same note never shift it.
        for (index, movement) in members.iter().zip(plan.moves.iter()) {
            let candidate = &report.candidates[*index];
            let absolute = bob_dir.join(dest);
            let dest_text = planner.peek_text(&absolute).ok_or_else(|| {
                CaptureError::io(format!(
                    "routing lost its destination note {}",
                    dest.display()
                ))
            })?;
            let new_id = movement.new_block_id.clone();
            let Some(root) = locate_dest_line(
                &dest_text,
                settings,
                new_id.as_deref(),
                &movement.title,
                candidate.status_symbol,
            ) else {
                return Err(CaptureError::io(format!(
                    "routing lost candidate {} ({}) in {}",
                    candidate.n,
                    candidate.key,
                    dest.display()
                )));
            };
            let root_line =
                dest_text.lines().nth(root).unwrap_or_default().to_string();
            let stamped = stamp_fresh(&root_line, today);
            if stamped.line != root_line {
                let mut rebuilt = dest_text
                    .lines()
                    .enumerate()
                    .map(|(number, line)| {
                        if number == root {
                            stamped.line.clone()
                        } else {
                            line.to_string()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if dest_text.ends_with('\n') {
                    rebuilt.push('\n');
                }
                planner.stage(&absolute, rebuilt)?;
            }
            let dest_relative = display_relative(dest);
            moves.push(RoutedTaskMove {
                candidate: *index,
                source: bob_dir.join(source),
                source_relative: display_relative(source),
                old_block_id: movement.old_block_id.clone(),
                old_line: movement.source_line,
                dest: absolute,
                dest_relative: dest_relative.clone(),
                dest_route: dest_relative
                    .strip_suffix(".md")
                    .unwrap_or(&dest_relative)
                    .to_string(),
                new_block_id: new_id,
                new_line: root + 1,
                repaired_links: group_links,
                repaired_dep_tokens: group_deps,
            });
        }
    }
    // Mark the report: moved candidates carry their answer, landing, and
    // outcome; keeps carry theirs; the rest stay pending.
    let mut routed_count = 0;
    let mut kept_count = 0;
    for (index, candidate) in report.candidates.iter_mut().enumerate() {
        match answered.get(&index) {
            Some(Some(_)) => {
                let movement = moves
                    .iter()
                    .find(|movement| movement.candidate == index)
                    .expect("routed candidate has a landing");
                let fresh = movement_fresh(planner, movement);
                candidate.answer = Some(InboxRouteAnswer {
                    action: "move",
                    destination: Some(movement.dest_relative.clone()),
                });
                candidate.outcome = "moved";
                candidate.moved = Some(serde_json::json!({
                    "destination": movement.dest_relative,
                    "line": movement.new_line,
                    "block_id": movement.new_block_id.clone().unwrap_or_default(),
                    "previous_block_id": movement.old_block_id.clone().unwrap_or_default(),
                    "fresh": fresh,
                }));
                candidate.repaired = Some((
                    movement.repaired_links,
                    movement.repaired_dep_tokens,
                ));
                routed_count += 1;
            }
            Some(None) | None if keep_all || answered.contains_key(&index) => {
                candidate.answer = Some(InboxRouteAnswer {
                    action: "keep",
                    destination: None,
                });
                candidate.outcome = "kept";
                kept_count += 1;
            }
            _ => {}
        }
    }
    if routed_count > 0 {
        report.repaired_files = Some(
            repaired
                .into_iter()
                .map(|(path, (link_count, dependency_token_count))| {
                    InboxRouteRepairedFile {
                        path: display_relative(&path),
                        link_count,
                        dependency_token_count,
                    }
                })
                .collect(),
        );
        report.renamed_block_ids = Some(renamed);
        report.needs_reconcile = Some(needs_reconcile);
    }
    report.routed_count = routed_count;
    report.kept_count = kept_count;
    Ok(AppliedInboxRoutes { report, moves })
}

/// The stamped date on a moved task's final line, or `None` when the
/// task is closed and was never stamped.
fn movement_fresh(
    planner: &CaptureBatchPlanner,
    movement: &RoutedTaskMove,
) -> Option<String> {
    let text = planner.peek_text(&movement.dest)?;
    let line = text.lines().nth(movement.new_line.saturating_sub(1))?;
    crate::native::task_fields::inline_fields(line, "fresh")
        .into_iter()
        .next()
        .map(|field| field.value)
}

/// Match one answer selector against the planned candidates: a
/// 1-based number or an opaque key.
fn resolve_route_selector(
    candidates: &[InboxRouteCandidate],
    selector: &str,
) -> Option<usize> {
    if selector.bytes().all(|byte| byte.is_ascii_digit())
        && let Ok(number) = selector.parse::<usize>()
        && (1..=candidates.len()).contains(&number)
    {
        return Some(number - 1);
    }
    candidates
        .iter()
        .position(|candidate| candidate.key == selector)
}

/// Resolve a route destination query like `bob task move`'s DEST (stem
/// or vault-relative path, with or without `.md`) and require it to be
/// a routable destination. Inbox notes and excluded folders refuse with
/// their own message; every other semantic refusal comes from the move
/// engine at plan time, so the list and the move can never disagree.
fn resolve_route_destination(
    snapshot: &VaultSnapshot,
    note_index: &NoteIndex,
    destinations: &[TaskMoveDestination],
    query: &str,
) -> Result<PathBuf, String> {
    if query.trim().is_empty()
        || target_to_markdown_path(query).is_none()
        || Path::new(query)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "note {query:?} is absolute, escaping, or otherwise not vault-relative"
        ));
    }
    let resolved = if query_is_explicit_route_path(query) {
        let Some(candidate) = target_to_markdown_path(query) else {
            return Err(format!(
                "note {query:?} is absolute, escaping, or otherwise not vault-relative"
            ));
        };
        let mut exact: Vec<PathBuf> = note_index
            .relative_paths()
            .filter(|path| paths_eq_route_ascii(path, &candidate))
            .map(Path::to_path_buf)
            .collect();
        exact.sort();
        exact.dedup();
        match exact.as_slice() {
            [path] => path.clone(),
            [] => {
                return Err(format!(
                    "note {query:?} was not found in the vault"
                ));
            }
            _ => {
                let mut listed = exact
                    .iter()
                    .map(|path| display_relative(path))
                    .collect::<Vec<_>>();
                listed.sort();
                return Err(format!(
                    "note {query:?} is ambiguous: {}",
                    listed.join(" ")
                ));
            }
        }
    } else {
        let mut found = note_index.basename_candidates(query);
        found.sort();
        found.dedup();
        match found.as_slice() {
            [path] => path.clone(),
            [] => {
                return Err(format!(
                    "note {query:?} was not found in the vault"
                ));
            }
            _ => {
                let mut listed = found
                    .iter()
                    .map(|path| display_relative(path))
                    .collect::<Vec<_>>();
                listed.sort();
                return Err(format!(
                    "note {query:?} is ambiguous: {}",
                    listed.join(" ")
                ));
            }
        }
    };
    let relative = display_relative(&resolved);
    if destinations.iter().any(|row| row.relative_path == relative) {
        return Ok(resolved);
    }
    let contents = snapshot.notes.get(&resolved);
    if contents.is_some_and(|text| is_inbox_note(&resolved, text, note_index)) {
        return Err(format!(
            "{relative} is an inbox note; routes go to areas and open projects"
        ));
    }
    if let Some(excluded) = excluded_route_dir(&resolved) {
        return Err(format!(
            "{relative} is under {excluded}/ and cannot receive routed tasks"
        ));
    }
    // Reachable only for notes the destination list excluded for engine
    // reasons (terminal projects, untyped notes, projects without Tasks):
    // hand them to the move so the refusal names the engine rule.
    Ok(resolved)
}

fn query_is_explicit_route_path(query: &str) -> bool {
    query.contains('/')
        || Path::new(query)
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

fn paths_eq_route_ascii(left: &Path, right: &Path) -> bool {
    left.as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
}

fn excluded_route_dir(path: &Path) -> Option<&'static str> {
    for component in path.components() {
        if let Component::Normal(name) = component
            && let Some(dir) = name.to_str()
            && matches!(dir, "done" | "_templates" | "_conflicts")
        {
            return Some(match dir {
                "done" => "done",
                "_templates" => "_templates",
                _ => "_conflicts",
            });
        }
    }
    None
}

/// Attribute a move-engine refusal to the group's first candidate, so
/// the card can reopen on it with the engine's message.
fn route_engine_refusal(
    report: &InboxRouteReport,
    members: &[usize],
    request: &MoveRequest,
    error: PlanError,
) -> CaptureError {
    let first = members
        .first()
        .and_then(|index| report.candidates.get(*index));
    match first {
        Some(candidate) => CaptureError::inbox_route_refused(
            report,
            candidate,
            request.dest.clone(),
            error.to_string(),
        ),
        None => CaptureError::io(error.to_string()),
    }
}

/// Relocate an id-less candidate's 0-based source line in the current
/// snapshot text. The recorded line wins when it still holds the
/// candidate; otherwise the exact pre-move text wins when unique;
/// otherwise a column-zero description-and-status scan wins when
/// unique. Block selections never relocate: block IDs are line-proof.
fn relocate_source_line(
    source_text: &str,
    settings: &note_tasks::NoteTaskSettings,
    candidate: &InboxRouteCandidate,
) -> Option<usize> {
    let scan = note_tasks::scan(source_text, settings);
    let expected = candidate.line.saturating_sub(1);
    if scan.task_at(expected).is_some_and(|task| {
        task.indentation.is_empty()
            && task.description == candidate.text
            && task.status_symbol == candidate.status_symbol
    }) {
        return Some(expected);
    }
    let lines: Vec<&str> = source_text.lines().collect();
    let mut exact = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if *line == candidate.root_line {
            exact.push(index);
        }
    }
    if exact.len() == 1 {
        return Some(exact[0]);
    }
    let scanned: Vec<usize> = scan
        .tasks()
        .iter()
        .filter(|task| {
            task.indentation.is_empty()
                && task.description == candidate.text
                && task.status_symbol == candidate.status_symbol
        })
        .map(|task| task.line_index)
        .collect();
    if scanned.len() == 1 {
        return Some(scanned[0]);
    }
    None
}

/// Locate a moved task's 0-based line in destination text: by its new
/// block ID when it has one, else by a unique column-zero
/// description-and-status match.
fn locate_dest_line(
    dest_text: &str,
    settings: &note_tasks::NoteTaskSettings,
    new_block_id: Option<&str>,
    title: &str,
    status: char,
) -> Option<usize> {
    let scan = note_tasks::scan(dest_text, settings);
    if let Some(id) = new_block_id.filter(|id| !id.is_empty())
        && let note_tasks::BlockIdLookup::Found(task) = scan.by_block_id(id)
    {
        return Some(task.line_index);
    }
    let matched: Vec<usize> = scan
        .tasks()
        .iter()
        .filter(|task| {
            task.indentation.is_empty()
                && task.description == title
                && task.status_symbol == status
        })
        .map(|task| task.line_index)
        .collect();
    (matched.len() == 1).then(|| matched[0])
}

fn pair_task_lines(
    original: &str,
    current: &str,
    original_scan: &note_tasks::NoteTaskScan,
    current_scan: &note_tasks::NoteTaskScan,
) -> BTreeMap<usize, usize> {
    use note_tasks::BlockIdLookup;

    let old_lines: Vec<&str> = original.lines().collect();
    let new_lines: Vec<&str> = current.lines().collect();
    let (old_to_new, _) = line_maps(&old_lines, &new_lines);
    let mut pairs = BTreeMap::new();
    let mut used_current = BTreeSet::new();

    // Stable block IDs remain the strongest identity when their physical
    // line was rewritten, such as a checkbox transition.
    for task in original_scan.tasks() {
        if let Some(id) = task.block_id.as_deref()
            && let BlockIdLookup::Found(found) = current_scan.by_block_id(id)
            && used_current.insert(found.line_index)
        {
            pairs.insert(task.line_index, found.line_index);
        }
    }
    // Exact unchanged lines handle ID-less tasks and status-independent
    // edits elsewhere in the note.
    for task in original_scan.tasks() {
        if pairs.contains_key(&task.line_index) {
            continue;
        }
        if let Some(line) = old_to_new.get(&task.line_index).copied()
            && current_scan.task_at(line).is_some()
            && used_current.insert(line)
        {
            pairs.insert(task.line_index, line);
        }
    }
    // A checkbox edit makes its physical row a Myers replacement rather
    // than an equal line. Pair the task rows inside one-to-one replacement
    // hunks; additions and deletions never become candidates.
    let mut hunk: Option<(std::ops::Range<usize>, std::ops::Range<usize>)> =
        None;
    for operation in similar::capture_diff_slices(
        similar::Algorithm::Myers,
        &old_lines,
        &new_lines,
    ) {
        if operation.tag() == similar::DiffTag::Equal {
            pair_hunk_tasks(
                hunk.take(),
                original_scan,
                current_scan,
                &mut pairs,
                &mut used_current,
            );
            continue;
        }
        let old_range = operation.old_range();
        let new_range = operation.new_range();
        if let Some((old, new)) = hunk.as_mut() {
            old.end = old.end.max(old_range.end);
            new.end = new.end.max(new_range.end);
        } else {
            hunk = Some((old_range, new_range));
        }
    }
    pair_hunk_tasks(
        hunk,
        original_scan,
        current_scan,
        &mut pairs,
        &mut used_current,
    );
    pairs
}

fn pair_hunk_tasks(
    hunk: Option<(std::ops::Range<usize>, std::ops::Range<usize>)>,
    original_scan: &note_tasks::NoteTaskScan,
    current_scan: &note_tasks::NoteTaskScan,
    pairs: &mut BTreeMap<usize, usize>,
    used_current: &mut BTreeSet<usize>,
) {
    let Some((old_range, new_range)) = hunk else {
        return;
    };
    let old_tasks: Vec<_> = original_scan
        .tasks()
        .iter()
        .filter(|task| {
            old_range.contains(&task.line_index)
                && !pairs.contains_key(&task.line_index)
        })
        .collect();
    let new_tasks: Vec<_> = current_scan
        .tasks()
        .iter()
        .filter(|task| {
            new_range.contains(&task.line_index)
                && !used_current.contains(&task.line_index)
        })
        .collect();
    let mut old_by_description = BTreeMap::<String, Vec<_>>::new();
    let mut new_by_description = BTreeMap::<String, Vec<_>>::new();
    for task in old_tasks.iter().filter(|task| task.block_id.is_none()) {
        old_by_description
            .entry(task.description.clone())
            .or_default()
            .push(*task);
    }
    for task in new_tasks.iter().filter(|task| task.block_id.is_none()) {
        new_by_description
            .entry(task.description.clone())
            .or_default()
            .push(*task);
    }
    let mut paired_old = BTreeSet::new();
    let mut paired_new = BTreeSet::new();
    for (description, old_matches) in old_by_description {
        let Some(new_matches) = new_by_description.get(&description) else {
            continue;
        };
        if old_matches.len() != new_matches.len() {
            continue;
        }
        for (old_task, new_task) in old_matches.into_iter().zip(new_matches) {
            pairs.insert(old_task.line_index, new_task.line_index);
            used_current.insert(new_task.line_index);
            paired_old.insert(old_task.line_index);
            paired_new.insert(new_task.line_index);
        }
    }
    let remaining_old: Vec<_> = old_tasks
        .into_iter()
        .filter(|task| !paired_old.contains(&task.line_index))
        .collect();
    let remaining_new: Vec<_> = new_tasks
        .into_iter()
        .filter(|task| !paired_new.contains(&task.line_index))
        .collect();
    if remaining_old.len() == remaining_new.len() {
        for (old_task, new_task) in remaining_old.into_iter().zip(remaining_new)
        {
            pairs.insert(old_task.line_index, new_task.line_index);
            used_current.insert(new_task.line_index);
        }
    }
}

fn column_zero_task_ancestor(
    lines: &[&str],
    scan: &note_tasks::NoteTaskScan,
    nested_line: usize,
) -> Option<usize> {
    let nested_start = line_start_offset(lines, nested_line);
    scan.tasks()
        .iter()
        .filter(|task| {
            task.line_index < nested_line
                && task.indentation.is_empty()
                && task.block_end > nested_start
                && lines
                    .get(task.line_index)
                    .is_some_and(|line| collect_done::has_task_tag(line))
        })
        .max_by_key(|task| task.line_index)
        .map(|task| task.line_index)
}

fn line_start_offset(lines: &[&str], line: usize) -> usize {
    lines.iter().take(line).map(|line| line.len() + 1).sum()
}

fn destination_snapshot(
    bob_dir: &Path,
    planner: &CaptureBatchPlanner,
) -> Result<VaultSnapshot, CaptureError> {
    let paths = collect_done::link_repair_markdown_files(bob_dir)
        .map_err(|error| CaptureError::io(error.to_string()))?;
    let mut notes = BTreeMap::new();
    for path in paths {
        let relative = path.strip_prefix(bob_dir).map_err(|error| {
            CaptureError::io(format!(
                "note {} is outside vault {}: {error}",
                path.display(),
                bob_dir.display()
            ))
        })?;
        let contents = fs::read_to_string(&path).map_err(|error| {
            CaptureError::io(format!(
                "failed to read {}: {error}",
                path.display()
            ))
        })?;
        notes.insert(relative.to_path_buf(), contents);
    }
    for file in &planner.files {
        let Ok(relative) = file.path.strip_prefix(bob_dir) else {
            continue;
        };
        if file.present {
            notes.insert(relative.to_path_buf(), file.current.clone());
        } else {
            notes.remove(relative);
        }
    }
    Ok(VaultSnapshot::from_notes(notes))
}

fn display_relative(path: &Path) -> String {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODAY: &str = "2026-10-24";

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().expect("parent directory"))
            .expect("create directory");
        fs::write(path, contents).expect("write note");
    }

    #[test]
    fn ir_nested_status_uses_root_and_idless_tasks_get_stable_keys() {
        // IR4: nested changes report their root; id-less duplicate keys are
        // numbered, and closing status changes remain route candidates.
        // IR5: a project filed under the inbox area is not itself an inbox.
        let temp = tempfile::tempdir().expect("temporary vault");
        let vault = temp.path().join("vault");
        write(&vault.join("inbox.md"), "# Inbox\n");
        let original = concat!(
            "---\n",
            "type: [[area]]\n",
            "parent: [[inbox]]\n",
            "---\n",
            "## Tasks\n",
            "- [ ] #task Parent task ^parent\n",
            "  - [ ] #task Nested child ^child\n",
            "- [*] #task Already Next ^next\n",
            "- [ ] #task Idless apples\n",
            "- [ ] #task Idless apples\n",
            "- [ ] #task Complete me ^complete\n",
        );
        let current = concat!(
            "---\n",
            "type: [[area]]\n",
            "parent: [[inbox]]\n",
            "---\n",
            "## Tasks\n",
            "- [ ] #task Parent task ^parent\n",
            "  - [*] #task Nested child ^child\n",
            "- [*] #task Already Next ^next\n",
            "- [/] #task Idless apples\n",
            "- [*] #task Idless apples\n",
            "- [x] #task Complete me ^complete\n",
            "- [ ] #task New this batch\n",
        );
        write(&vault.join("mac_inbox.md"), original);
        write(&vault.join("health.md"), "---\ntype: [[area]]\n---\n");
        let project_before = concat!(
            "---\n",
            "type: [[project]]\n",
            "status: wip\n",
            "parent: [[mac_inbox]]\n",
            "---\n",
            "## Tasks\n",
            "- [ ] #task Project task ^project-task\n",
        );
        let project_after = project_before
            .replace("- [ ] #task Project task", "- [*] #task Project task");
        write(&vault.join("projects/under-inbox.md"), project_before);

        let settings = note_tasks::read_settings(&vault);
        let mut planner = CaptureBatchPlanner::default();
        let inbox_path = vault.join("mac_inbox.md");
        planner.ensure_loaded(&inbox_path).expect("load inbox");
        planner
            .stage(&inbox_path, current.to_string())
            .expect("stage inbox update");
        let project_path = vault.join("projects/under-inbox.md");
        planner.ensure_loaded(&project_path).expect("load project");
        planner
            .stage(&project_path, project_after)
            .expect("stage project update");
        let today = chrono::NaiveDate::parse_from_str(TODAY, "%Y-%m-%d")
            .expect("today");
        let dependency_ctx = DependencyContext::new(&vault, today);
        let report =
            detect_inbox_routes(&planner, &vault, &settings, &dependency_ctx)
                .expect("route report");

        assert_eq!(report.candidate_count, 4, "{report:#?}");
        assert_eq!(report.candidates[0].key, "mac_inbox.md#^parent");
        assert_eq!(report.candidates[0].line, 6);
        assert_eq!(report.candidates[0].text, "Parent task");
        assert_eq!(report.candidates[0].previous_status_symbol, ' ');
        assert_eq!(report.candidates[0].status_symbol, ' ');
        let digest =
            hex::encode(Sha256::digest("Idless apples"))[..8].to_string();
        assert_eq!(report.candidates[1].key, format!("mac_inbox.md@{digest}"));
        assert_eq!(report.candidates[1].previous_status_symbol, ' ');
        assert_eq!(report.candidates[1].status_symbol, '/');
        assert_eq!(report.candidates[1].n, 2);
        assert_eq!(
            report.candidates[2].key,
            format!("mac_inbox.md@{digest}~2")
        );
        assert_eq!(report.candidates[2].status_symbol, '*');
        assert_eq!(report.candidates[3].key, "mac_inbox.md#^complete");
        assert_eq!(report.candidates[3].status_symbol, 'x');
        assert_eq!(report.candidates[0].answer, None);
        assert_eq!(report.candidates[0].outcome, "pending");
        let paths: Vec<_> = report
            .destinations
            .as_ref()
            .expect("destinations")
            .iter()
            .map(|destination| destination.relative_path.as_str())
            .collect();
        assert!(paths.contains(&"health.md"));
        assert!(paths.contains(&"projects/under-inbox.md"));
        assert!(!paths.contains(&"mac_inbox.md"));
        assert!(!paths.contains(&"inbox.md"));
    }

    #[test]
    fn ir_routed_batch_rolls_back_when_a_repaired_file_changes() {
        // IRA15: route postimages stage through the planner, so the
        // commit's disk-preimage guard covers the link-repaired files: a
        // concurrent edit aborts the whole batch with the standard
        // NoteChanged failure and no file is written.
        let temp = tempfile::tempdir().expect("temporary vault");
        let vault = temp.path().join("vault");
        write(
            &vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
            concat!(
                "{\"globalFilter\": \"#task\",",
                "\"statusSettings\": {\"coreStatuses\": [",
                "{\"symbol\":\" \",\"name\":\"Todo\",\"type\":\"TODO\"},",
                "{\"symbol\":\"x\",\"name\":\"Done\",\"type\":\"DONE\"}],",
                "\"customStatuses\": [",
                "{\"symbol\":\"/\",\"name\":\"In Progress\",\"type\":\"IN_PROGRESS\"},",
                "{\"symbol\":\"*\",\"name\":\"Next\",\"type\":\"ON_HOLD\"},",
                "{\"symbol\":\"?\",\"name\":\"Blocked\",\"nextStatusSymbol\":\" \",\"availableAsCommand\":true,\"type\":\"ON_HOLD\"},",
                "{\"symbol\":\"-\",\"name\":\"Canceled\",\"type\":\"CANCELLED\"}]}}",
            ),
        );
        write(&vault.join("inbox.md"), "# Inbox\n");
        let inbox_before = "---\ntype: [[area]]\nparent: [[inbox]]\n---\n## Tasks\n- [ ] #task Route me ^route-task\n";
        write(&vault.join("mac_inbox.md"), inbox_before);
        let health_before = "---\ntype: [[area]]\n---\n## Tasks\n";
        write(&vault.join("health.md"), health_before);
        write(
            &vault.join("dev.md"),
            "---\ntype: [[area]]\n---\n## Tasks\n- [ ] #task Prereq ^prereq\n",
        );
        let day_path = crate::native::pomodoro::day_file_for(&vault);
        write(&day_path, "## Pomodoros\n- [ ] (**0900-0930**) — WORK\n");
        let request = CaptureRequest {
            bob_dir: vault.clone(),
            dry_run: true,
            forced_clip: None,
            forced_destination_flags: Vec::new(),
            forced_route: None,
            forced_section: None,
            forced_sub_bullet_target: None,
            forced_task_section: None,
            inbox_routes: vec![InboxRoutePick::Answer {
                selector: "1".to_string(),
                destination: Some("health".to_string()),
            }],
            no_clip: true,
            no_ref: false,
            pull_in: None,
            refuse_override: false,
            require_inbox_route_choice: false,
            require_pull_in_choice: false,
            template_origin: None,
            raw_text: "^mac_inbox:route-task &dev:prereq".to_string(),
            selection: crate::native::task_complete::SuccessorSelection::None,
        };
        let mut batch =
            plan_capture_batch(&request, None).expect("plan routes");
        assert_eq!(batch.inbox_route.routed_count, 1);
        assert!(
            batch
                .inbox_route
                .repaired_files
                .as_ref()
                .is_some_and(|files| files
                    .iter()
                    .any(|file| file.path == "dev.md" && file.link_count > 0)),
            "{:#?}",
            batch.inbox_route.repaired_files
        );
        // A concurrent edit lands in a repaired file after planning.
        let concurrent = format!(
            "{}- [ ] #task Concurrent ^concurrent\n",
            fs::read_to_string(vault.join("dev.md")).expect("dev note")
        );
        write(&vault.join("dev.md"), &concurrent);
        let error = commit_capture_batch(&mut batch, &vault)
            .expect_err("commit refuses");
        assert_eq!(error.typed, Some(CaptureFailureKind::NoteChanged));
        assert!(
            error
                .message
                .contains("note changed on disk after planning"),
            "{}",
            error.message
        );
        // Nothing was written: source and destination are byte-identical,
        // and the repaired file keeps only the concurrent edit.
        assert_eq!(
            fs::read_to_string(vault.join("mac_inbox.md")).expect("inbox note"),
            inbox_before
        );
        assert_eq!(
            fs::read_to_string(vault.join("health.md")).expect("health note"),
            health_before
        );
        assert_eq!(
            fs::read_to_string(vault.join("dev.md")).expect("dev note"),
            concurrent
        );
    }
}

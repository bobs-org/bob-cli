//! Note-pull planner for lone `@route` captures
//! (`docs/task-dependencies.md` §12.20).
//!
//! A lone task-list header offers its note's eligible open residents
//! through the Pull-In protocol. Rows have trigger `note_pull`, no
//! predecessor, and normal typed placement without an anchor.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use crate::native::capture_dependency_tasks::{
    has_hide_tag, has_prj_is_blocked_tag,
};
use crate::native::capture_task_toggle::set_task_line_status;
use crate::native::freshness::placement::stamp_fresh;
use crate::native::note_tasks::{clean_description, stable_task_key};
use crate::native::task_complete::{
    is_inbox_note, live_link_keys, OfferGroup, SuccessorSelection,
    NOTE_PULL_TRIGGER,
};
use crate::native::task_status_hooks::{self, FileScan};

use super::batch::CaptureBatchPlanner;
use super::dependencies::DependencyContext;
use super::output::{
    CaptureError, CaptureItemResult, NotePullSummaryJson, Placement,
    PullInFoldedJson, TaskCompleteOfferDestinationJson, TaskCompleteOfferJson,
    TaskCompleteSuccessorLinkJson, TaskCompleteUnblockedJson,
};
use super::plan::{capture_kind_label, PlannedCaptureItem};
use super::task_theme::CaptureTaskThemeContext;
use super::{CaptureRequest, CaptureWarning};

/// Plan one lone-`@route` item: classify residents, compute keys, place,
/// apply explicit picks, and report the additive `note_pull` payload.
#[allow(clippy::too_many_arguments)]
pub(super) fn plan_note_pull_item(
    request: &CaptureRequest,
    parsed: crate::native::capture_language::ParsedCaptureText,
    today: NaiveDate,
    planner: &mut CaptureBatchPlanner,
    dependency_ctx: &mut DependencyContext,
    task_theme_context: &mut CaptureTaskThemeContext,
    warnings: &mut Vec<CaptureWarning>,
) -> Result<PlannedCaptureItem, CaptureError> {
    let route = match &parsed.kind {
        crate::native::capture_language::CaptureKind::NotePull { route } => {
            route.clone()
        }
        _ => {
            return Err(CaptureError::io(
                "note-pull planner invariant failed: wrong capture kind",
            ));
        }
    };
    let note = format!("{route}.md");
    let relative = PathBuf::from(&note);
    let absolute = request.bob_dir.join(&relative);

    let Some(source_text) = planner.current_contents(&absolute)? else {
        return Err(pull_note_missing_error(&route, &note));
    };

    let day_file = crate::native::pomodoro::day_file_for(&request.bob_dir);
    if !planner.currently_exists(&day_file)? {
        return Err(CaptureError::io(format!(
            "Bob daily note does not exist: {}",
            day_file.display()
        )));
    }
    let day_text = planner.read_existing(&day_file)?;
    let day_relative = crate::native::capture_pomodoros::relative_day_file(
        &day_file,
        &request.bob_dir,
    );

    let tasks_settings =
        crate::native::task_status_hooks::read_tasks_settings(&request.bob_dir);
    let note_settings =
        crate::native::note_tasks::read_settings(&request.bob_dir);
    let tasks = task_status_hooks::parse_tasks(&source_text, &tasks_settings);

    let index = super::task_complete::staged_note_index(
        &request.bob_dir,
        planner,
        dependency_ctx,
    );
    let live = live_link_keys(&day_text, Path::new(&day_relative), &index);
    let inbox = is_inbox_note(&relative, &source_text, &index);

    // Prerequisite proof is needed only when a candidate carries a
    // non-empty `[dependsOn::]` (the KM31 gate).
    let needs_snapshot = tasks.iter().any(|task| !task.depends_on.is_empty());
    let snapshot_state: Option<SnapshotProof> = if needs_snapshot {
        Some(prepare_snapshot_proof(
            dependency_ctx,
            planner,
            &request.bob_dir,
            &tasks_settings,
        ))
    } else {
        None
    };

    let mut folded_already = 0usize;
    let mut folded_blocked = 0usize;
    let mut folded_scheduled = 0usize;
    let mut folded_hidden = 0usize;
    let mut offers: Vec<NotePullOffer> = Vec::new();

    for task in &tasks {
        let raw = crate::native::task_complete::successors::raw_task_line(
            &source_text,
            task.line_index,
        );
        if task.block_id.as_deref() == Some("prj") {
            continue;
        }
        if !matches!(task.status, ' ' | '*' | '/' | '?') {
            continue;
        }
        if !task.status_recognized {
            continue;
        }
        if task.status == '?' || has_prj_is_blocked_tag(&raw) {
            folded_blocked += 1;
            continue;
        }
        if !task.depends_on.is_empty() {
            let blocked = match snapshot_state.as_ref() {
                None => true,
                Some(proof) => proof.is_blocked(&relative, task),
            };
            if blocked {
                folded_blocked += 1;
                continue;
            }
        }
        if task.scheduled.is_some_and(|date| date > today) {
            folded_scheduled += 1;
            continue;
        }
        if has_hide_tag(&raw) {
            folded_hidden += 1;
            continue;
        }
        if let Some(block_id) = task.block_id.as_deref()
            && live.contains(&(relative.clone(), block_id.to_string()))
        {
            folded_already += 1;
            continue;
        }
        if !matches!(task.status, ' ' | '*' | '/') {
            continue;
        }
        offers.push(NotePullOffer {
            line_index: task.line_index,
            status: task.status,
            block_id: task.block_id.clone(),
            depends_on_empty: task.depends_on.is_empty(),
            text: clean_description(
                &task.description,
                &note_settings.global_filter,
                task.block_id.as_deref(),
            ),
            raw_status_line: raw.to_string(),
        });
    }

    if offers.is_empty() {
        return Err(nothing_to_pull_in_error(
            &tasks,
            &note,
            &route,
            today,
            folded_already,
            folded_blocked,
            folded_scheduled,
            folded_hidden,
        ));
    }

    // Freeze keys from the staged source before any pick.
    let mut ordinal_seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut frozen: Vec<FrozenNotePull> = Vec::new();
    for offer in &offers {
        let digest_index = if offer.block_id.is_some() {
            1
        } else {
            let count = ordinal_seen.entry(offer.text.clone()).or_insert(0);
            *count += 1;
            *count
        };
        let key = stable_task_key(
            &relative,
            offer.block_id.as_deref(),
            &offer.text,
            &note_settings.global_filter,
            digest_index,
        );
        let selected = match &request.selection {
            SuccessorSelection::All => true,
            SuccessorSelection::None => false,
            SuccessorSelection::Keys(keys) => keys.contains(&key),
        };
        frozen.push(FrozenNotePull {
            key,
            status: offer.status,
            text: offer.text.clone(),
            block_id: offer.block_id.clone(),
            block_id_needed: offer.block_id.is_none(),
            selected,
            line_index: offer.line_index,
            order_lane:
                crate::native::task_complete::keep_moving::keep_moving_lane(
                    offer.status,
                ),
            raw: offer.raw_status_line.clone(),
        });
    }
    // Lane pull order (Pending → Next → Ready), then source line.
    frozen.sort_by(|left, right| {
        left.order_lane
            .cmp(&right.order_lane)
            .then(left.line_index.cmp(&right.line_index))
    });

    // Sequential scratch placement through all offers so a second DOCS row
    // sees the placeholder the first would create. Provisional IDs stand in
    // for ID-less rows; they are never staged or reported as minted.
    let mut scratch_day = day_text.clone();
    let mut scratch_probe: HashSet<String> =
        crate::native::collect_done::block_ids_in_markdown(&scratch_day)
            .into_iter()
            .collect();
    scratch_probe.extend(crate::native::collect_done::block_ids_in_markdown(
        &source_text,
    ));
    let mut scratch_dests: Vec<ScratchDest> = Vec::new();
    for offer in &frozen {
        let block_id = offer.block_id.clone().unwrap_or_else(|| {
            let used: HashSet<&str> =
                scratch_probe.iter().map(String::as_str).collect();
            let minted = crate::native::capture_block_ids::mint_block_id(
                &offer.text,
                &used,
            );
            scratch_probe.insert(minted.clone());
            minted
        });
        let block_link =
            format!("[[{}#^{block_id}]]", note.trim_end_matches(".md"));
        let placement_type = task_theme_context
            .placement_for_task(planner, &route, &offer.raw, warnings);
        let typed_pair = placement_type
            .as_ref()
            .map(|placement| (placement.as_typed(), placement.placed_type()));
        let (typed, _placed_type) = match typed_pair.as_ref() {
            Some((typed, placed_type)) => {
                (Some(typed), Some(placed_type.clone()))
            }
            None => (None, None),
        };
        let insertion = super::pomodoro_insert::insert_pomodoro_block_link(
            &scratch_day,
            &block_link,
            None,
            false,
            typed,
        )?;
        let creates = insertion.creates_pomodoro;
        let used_typed = typed.is_some();
        scratch_day = insertion.updated.clone();
        scratch_dests.push(ScratchDest {
            entry_name: insertion.destination.name.clone().unwrap_or_default(),
            entry_role: if creates {
                "created"
            } else if used_typed {
                "typed"
            } else {
                "implicit"
            },
            fallback: !creates && !used_typed,
            creates,
        });
    }

    // Actual placement processes only picks; an unpicked row cannot create
    // a placeholder.
    let mut actual_day = day_text.clone();
    let mut actual_by_key: BTreeMap<String, ActualDest> = BTreeMap::new();
    for offer in frozen.iter().filter(|offer| offer.selected) {
        let block_id = match offer.block_id.clone() {
            Some(id) => id,
            None => super::blocks_mirror::mint_staged_block_id(
                dependency_ctx,
                planner,
                &absolute,
                &relative,
                &offer.raw,
            )?,
        };
        let block_link =
            format!("[[{}#^{block_id}]]", note.trim_end_matches(".md"));
        let placement_type = task_theme_context
            .placement_for_task(planner, &route, &offer.raw, warnings);
        let typed_pair = placement_type
            .as_ref()
            .map(|placement| (placement.as_typed(), placement.placed_type()));
        let (typed, placed_type) = match typed_pair.as_ref() {
            Some((typed, placed_type)) => {
                (Some(typed), Some(placed_type.clone()))
            }
            None => (None, None),
        };
        let mut insertion = super::pomodoro_insert::insert_pomodoro_block_link(
            &actual_day,
            &block_link,
            None,
            false,
            typed,
        )?;
        if let Some(placed_type) = placed_type {
            insertion.destination.placed_type = Some(placed_type);
        }
        let creates = insertion.creates_pomodoro;
        let used_typed = typed.is_some();
        actual_day = insertion.updated.clone();
        actual_by_key.insert(
            offer.key.clone(),
            ActualDest {
                updated: actual_day.clone(),
                insertion,
                block_id,
                creates,
                used_typed,
            },
        );
    }

    // Apply source edits for picks: status, mint, fresh stamp. Schedules,
    // `[id::]`, and `[dependsOn::]` are untouched. Same-draft creation is
    // never stamped; single-item pulls have no creation provenance, so
    // every pick stamps fresh here (the batch phase owns the exemption).
    let mut edited_lines: Vec<String> =
        source_text.split('\n').map(str::to_string).collect();
    // Track final block IDs for reporting.
    let mut final_ids: BTreeMap<String, String> = BTreeMap::new();
    for offer in frozen.iter().filter(|offer| offer.selected) {
        let actual = actual_by_key.get(&offer.key).expect("placed pick");
        final_ids.insert(offer.key.clone(), actual.block_id.clone());
        let Some(line) = edited_lines.get_mut(offer.line_index) else {
            continue;
        };
        let trimmed = line.strip_suffix('\r').unwrap_or(line).to_string();
        let new_status = match offer.status {
            ' ' => '*',
            other => other,
        };
        let mut updated =
            set_task_line_status(&trimmed, new_status).unwrap_or(trimmed);
        if offer.block_id.is_none() {
            updated.push_str(" ^");
            updated.push_str(&actual.block_id);
        }
        updated = stamp_fresh(&updated, today).line;
        if line.ends_with('\r') {
            updated.push('\r');
        }
        *line = updated;
    }
    if frozen.iter().any(|offer| offer.selected) {
        planner.stage(&absolute, edited_lines.join("\n"))?;
        planner.stage(&day_file, actual_day.clone())?;
    }

    // Touched source rows report task blocks with the note-pull role so
    // the batch tracker renders each picked task once, in first-touch
    // order, in its final state.
    let task_block_refs: Vec<super::task_blocks::TaskBlockRef> = frozen
        .iter()
        .filter(|offer| offer.selected)
        .filter_map(|offer| {
            let block_id = final_ids
                .get(&offer.key)
                .cloned()
                .or_else(|| offer.block_id.clone())?;
            Some(super::task_blocks::TaskBlockRef {
                target: absolute.clone(),
                relative_target: note.clone(),
                route: route.clone(),
                line: offer.line_index,
                block_id: Some(block_id),
                role: super::task_blocks::TaskBlockRole::NotePull,
            })
        })
        .collect();

    let day_file_display = day_relative.clone();
    let mut successors: Vec<TaskCompleteUnblockedJson> = Vec::new();
    for (position, offer) in frozen.iter().enumerate() {
        let block_id = final_ids
            .get(&offer.key)
            .cloned()
            .or_else(|| offer.block_id.clone())
            .unwrap_or_default();
        let new_status = match offer.status {
            ' ' => '*',
            other => other,
        };
        let (link, not_linked, destination, _order_entry) = if offer.selected {
            let actual = actual_by_key.get(&offer.key).expect("placed pick");
            let dest = &actual.insertion.destination;
            let entry_name = dest.name.clone().unwrap_or_default();
            let (entry_role, fallback) = if actual.creates {
                ("created", false)
            } else if actual.used_typed {
                ("typed", false)
            } else {
                ("implicit", true)
            };
            let stem = note.trim_end_matches(".md");
            let block_link = format!("[[{stem}#^{block_id}]]");
            let line_no = block_link_line(&actual.updated, &block_link);
            (
                Some(TaskCompleteSuccessorLinkJson {
                    day_file: day_file_display.clone(),
                    entry_name: entry_name.clone(),
                    entry_line: dest.line,
                    entry_created: actual.insertion.creates_pomodoro,
                    next_up: dest.role == Some("next_up"),
                    line: line_no,
                    block_link,
                    block_id_created: offer.block_id_needed,
                }),
                None,
                TaskCompleteOfferDestinationJson {
                    day_file: day_file_display.clone(),
                    entry_name,
                    entry_role,
                    fallback,
                },
                dest.line,
            )
        } else {
            let scratch = &scratch_dests[position];
            (
                None,
                Some("not_selected".to_string()),
                TaskCompleteOfferDestinationJson {
                    day_file: day_file_display.clone(),
                    entry_name: scratch.entry_name.clone(),
                    entry_role: scratch.entry_role,
                    fallback: scratch.fallback,
                },
                0,
            )
        };
        let order_key: crate::native::task_complete::OfferOrderKey = (
            OfferGroup::NotePull as u8,
            (usize::MAX, usize::MAX),
            offer.order_lane,
            relative.clone(),
            offer.line_index,
        );
        successors.push(TaskCompleteUnblockedJson {
            note_path: note.clone(),
            block_id: block_id.clone(),
            line: offer.line_index + 1,
            text: offer.text.clone(),
            previous_status_symbol: offer.status,
            previous_status_name: super::task_complete::status_name_for(
                &request.bob_dir,
                offer.status,
            ),
            status_symbol: if offer.selected {
                new_status
            } else {
                offer.status
            },
            status_name: super::task_complete::status_name_for(
                &request.bob_dir,
                if offer.selected {
                    new_status
                } else {
                    offer.status
                },
            ),
            inbox,
            unblocked_by: Vec::new(),
            link,
            not_linked,
            trigger: NOTE_PULL_TRIGGER,
            hide_removed: false,
            offer: Some(TaskCompleteOfferJson {
                key: offer.key.clone(),
                n: position + 1,
                group: "note_pull",
                selected: offer.selected,
                block_id_needed: offer.block_id_needed,
                project: None,
                after: None,
                destination,
                order_key,
                settled_ordinal: None,
            }),
            relation: None,
        });
    }

    let folded = PullInFoldedJson {
        hidden: folded_hidden,
        already_planned: folded_already,
        sub_projects: 0,
        blocked: folded_blocked,
        scheduled: folded_scheduled,
        already_started: 0,
        waits_for: 0,
    };

    let kind_label = capture_kind_label(&parsed.kind);
    Ok(PlannedCaptureItem {
        result: CaptureItemResult {
            ok: true,
            dry_run: request.dry_run,
            routed: true,
            route: Some(route.clone()),
            route_label: route.clone(),
            relative_target: note.clone(),
            target: absolute.display().to_string(),
            text: String::new(),
            task_line: String::new(),
            task_theme: None,
            type_relocation: None,
            kind: kind_label,
            created: super::plan::date_string(today),
            scheduled: None,
            priority: None,
            priority_label: None,
            placement: Placement::Created,
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
            pomodoro_reset: None,
            dependency_update: None,
            consumed_links: Vec::new(),
            task_complete: None,
            r#ref: None,
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
            list: None,
            relation_successors: Vec::new(),
            relation_held: Vec::new(),
            relation_check: None,
            relation_groups: Vec::new(),
            relation_label_hides: Vec::new(),
            note_pull: Some(NotePullSummaryJson {
                note: note.clone(),
                route: route.clone(),
                successors,
                folded,
            }),
        },
        clip_plan: None,
        pomodoro_refs: Vec::new(),
        task_block_refs,
    })
}

struct NotePullOffer {
    line_index: usize,
    status: char,
    block_id: Option<String>,
    #[allow(dead_code)]
    depends_on_empty: bool,
    text: String,
    #[allow(dead_code)]
    raw_status_line: String,
}

struct FrozenNotePull {
    key: String,
    status: char,
    text: String,
    block_id: Option<String>,
    block_id_needed: bool,
    selected: bool,
    line_index: usize,
    order_lane: u8,
    raw: String,
}

struct ScratchDest {
    entry_name: String,
    entry_role: &'static str,
    fallback: bool,
    #[allow(dead_code)]
    creates: bool,
}

struct ActualDest {
    updated: String,
    insertion: super::pomodoro_insert::PomodoroBlockLinkInsertion,
    block_id: String,
    creates: bool,
    used_typed: bool,
}

#[allow(dead_code)]
fn offer_text_line(offer: &FrozenNotePull) -> String {
    if let Some(id) = offer.block_id.as_deref() {
        format!("- [{}] {} ^{id}", offer.status, offer.text)
    } else {
        format!("- [{}] {}", offer.status, offer.text)
    }
}

fn block_link_line(contents: &str, block_link: &str) -> usize {
    contents
        .lines()
        .position(|line| line.contains(block_link))
        .map(|index| index + 1)
        .unwrap_or(0)
}

/// Vault-wide prerequisite proof for dependency-bearing candidates,
/// honoring the KM31 gate: only built when needed, overlaid with staged
/// text, with unavailable snapshots folding candidates as blocked.
struct SnapshotProof {
    files: Vec<FileScan>,
    states: BTreeMap<(usize, usize), task_status_hooks::TaskDependencyState>,
}

impl SnapshotProof {
    fn is_blocked(
        &self,
        relative: &Path,
        task: &task_status_hooks::TaskLine,
    ) -> bool {
        let Some(source) = self
            .files
            .iter()
            .position(|file| file.relative_path == relative)
        else {
            return true;
        };
        let Some(state) = self.states.get(&(source, task.line_index)) else {
            return true;
        };
        !crate::native::project_dependencies::blocking_open_dependency_ids(
            &self.files[source],
            task,
            &state.open_dependency_ids,
        )
        .is_empty()
    }
}

fn prepare_snapshot_proof(
    dependency_ctx: &DependencyContext,
    planner: &CaptureBatchPlanner,
    bob_dir: &Path,
    tasks_settings: &task_status_hooks::TasksSettings,
) -> SnapshotProof {
    let staged = super::task_complete::staged_overlay_map(bob_dir, planner);
    let snapshot = dependency_ctx.dependents_snapshot();
    if !snapshot.snapshot_complete(&staged) {
        return SnapshotProof {
            files: Vec::new(),
            states: BTreeMap::new(),
        };
    }
    let overlaid = snapshot.overlaid(staged);
    let files: Vec<FileScan> = overlaid
        .into_iter()
        .map(|(relative_path, contents)| {
            let tasks =
                task_status_hooks::parse_tasks(&contents, tasks_settings);
            FileScan {
                path: bob_dir.join(&relative_path),
                relative_path,
                contents,
                tasks,
            }
        })
        .collect();
    let states =
        task_status_hooks::task_dependency_states(&files, &BTreeSet::new());
    SnapshotProof { files, states }
}

fn pull_note_missing_error(route: &str, note: &str) -> CaptureError {
    CaptureError::usage_code(
        format!(
            "@{route} pulls ready tasks from {note}, which does not exist: \
             add `- <task>` bullets below it to start a list there, or run \
             'bob capture-targets' to list routable notes"
        ),
        "pull_note_missing",
    )
}

#[allow(clippy::too_many_arguments)]
fn nothing_to_pull_in_error(
    tasks: &[task_status_hooks::TaskLine],
    note: &str,
    route: &str,
    today: NaiveDate,
    already: usize,
    blocked: usize,
    scheduled: usize,
    hidden: usize,
) -> CaptureError {
    let mut extra = serde_json::Map::new();
    extra.insert(
        "note_pull".to_string(),
        serde_json::to_value(&NotePullSummaryJson {
            note: note.to_string(),
            route: route.to_string(),
            successors: Vec::new(),
            folded: PullInFoldedJson {
                hidden,
                already_planned: already,
                sub_projects: 0,
                blocked,
                scheduled,
                already_started: 0,
                waits_for: 0,
            },
        })
        .expect("serialize note_pull"),
    );
    let message = if blocked > 0
        && let Some(reason) = blocked_project_reason(tasks, today)
    {
        format!(
            "nothing to pull in from {note}: the project is blocked ({reason})"
        )
    } else if already + blocked + scheduled + hidden > 0 {
        let mut parts: Vec<String> = Vec::new();
        if already > 0 {
            parts.push(format!("{already} already planned"));
        }
        if blocked > 0 {
            parts.push(format!("{blocked} blocked"));
        }
        if scheduled > 0 {
            parts.push(format!("{scheduled} scheduled later"));
        }
        if hidden > 0 {
            parts.push(format!("{hidden} hidden"));
        }
        format!("nothing to pull in from {note}: {}", parts.join(" · "))
    } else {
        format!("nothing to pull in from {note}: it has no open tasks")
    };
    let mut error = CaptureError::usage_code(message, "nothing_to_pull_in");
    error.extra = Some(extra);
    error
}

fn blocked_project_reason(
    tasks: &[task_status_hooks::TaskLine],
    today: NaiveDate,
) -> Option<String> {
    let prj = tasks
        .iter()
        .find(|task| task.block_id.as_deref() == Some("prj"))?;
    if prj.scheduled.is_some_and(|date| date > today) {
        let date = prj
            .scheduled
            .map(|date| date.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        return Some(format!("is scheduled for {date}"));
    }
    if !prj.depends_on.is_empty() {
        let count = prj.depends_on.len();
        let task_word = if count == 1 { "task" } else { "tasks" };
        return Some(format!("its ^prj waits on {count} {task_word}"));
    }
    if prj.status == '?' {
        return Some("its ^prj is blocked".to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::capture_language::{CaptureKind, ParsedCaptureText};
    use crate::native::env::TestEnvGuard;
    use crate::native::task_complete::SuccessorSelection;
    use std::fs;

    const TODAY_YMD: (i32, u32, u32) = (2026, 9, 20);

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(TODAY_YMD.0, TODAY_YMD.1, TODAY_YMD.2)
            .expect("valid test date")
    }

    struct Harness {
        _temp: tempfile::TempDir,
        _guard: TestEnvGuard,
        request: CaptureRequest,
        planner: CaptureBatchPlanner,
        dependency_ctx: DependencyContext,
        task_theme_context: CaptureTaskThemeContext,
        warnings: Vec<CaptureWarning>,
    }

    fn vault_harness(
        files: &[(&str, &str)],
        day: Option<&str>,
        selection: SuccessorSelection,
    ) -> Harness {
        let temp = tempfile::tempdir().expect("create note-pull vault");
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).expect("create vault");
        for (name, contents) in files {
            let path = vault.join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(&path, contents).expect("write fixture note");
        }
        let day_file = vault.join("day.md");
        if let Some(day) = day {
            fs::write(&day_file, day).expect("write day file");
        }
        // Tasks status knowledge mirrors the capture offer harness:
        // open lanes plus the blocked marker status.
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
        // Isolate plan config so theme placement never reads the host's
        // real config: a missing file loads `PlanConfig::default()`.
        let missing_config = temp.path().join("missing-bob-config.toml");
        let guard = TestEnvGuard::set(&[
            ("BOB_NOW", Some(std::ffi::OsStr::new("2026-09-20 14:31:07"))),
            ("BOB_DAY_FILE", Some(day_file.as_os_str())),
            ("BOB_CONFIG_FILE", Some(missing_config.as_os_str())),
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
            raw_text: "@sase".to_string(),
            selection,
        };
        let today_date = today();
        Harness {
            planner: CaptureBatchPlanner::default(),
            dependency_ctx: DependencyContext::new(
                &request.bob_dir,
                today_date,
            ),
            task_theme_context: CaptureTaskThemeContext::new(
                &request.bob_dir,
                today_date,
                None,
            ),
            warnings: Vec::new(),
            request,
            _temp: temp,
            _guard: guard,
        }
    }

    fn parsed_pull() -> ParsedCaptureText {
        ParsedCaptureText {
            body: String::new(),
            clip: None,
            route: Some("sase".to_string()),
            kind: CaptureKind::NotePull {
                route: "sase".to_string(),
            },
            scheduled_offset: None,
            priority_level: None,
            sub_bullets: Vec::new(),
            dependencies: Vec::new(),
            blocked_tasks: Vec::new(),
            relations: Vec::new(),
            dependency_target: None,
            type_marker: None,
        }
    }

    fn plan(harness: &mut Harness) -> Result<PlannedCaptureItem, CaptureError> {
        let parsed = parsed_pull();
        plan_note_pull_item(
            &harness.request,
            parsed,
            today(),
            &mut harness.planner,
            &mut harness.dependency_ctx,
            &mut harness.task_theme_context,
            &mut harness.warnings,
        )
    }

    fn staged(harness: &Harness, name: &str) -> Option<String> {
        harness
            .planner
            .peek_text(&harness.request.bob_dir.join(name))
    }

    fn summary(planned: &PlannedCaptureItem) -> &NotePullSummaryJson {
        planned
            .result
            .note_pull
            .as_ref()
            .expect("note_pull payload")
    }

    const FIX_DAY: &str = "## Pomodoros\n- [ ] () \u{2014} FIX\n";

    const PBA: &str = concat!(
        "- [/] #task Ship the badge renderer [id:: p] ^ship-renderer\n",
        "- [*] #task Wire badges into the README [id:: b] ^readme-badges\n",
        "- [ ] #task Write badge docs [id:: a]\n",
    );

    // NP1: one `note_pull` group orders Pending, Next, Ready; rows carry
    // `trigger: note_pull` with empty `unblocked_by`; nothing is written.
    #[test]
    fn np1_no_pick_reports_lane_order_and_writes_nothing() {
        let mut harness = vault_harness(
            &[("sase.md", PBA)],
            Some(FIX_DAY),
            SuccessorSelection::None,
        );
        let planned = plan(&mut harness).expect("NP1 plans");
        assert!(planned.result.ok);
        assert_eq!(planned.result.kind, "note_pull");
        let pull = summary(&planned);
        assert_eq!(pull.note, "sase.md");
        assert_eq!(pull.route, "sase");
        assert_eq!(pull.successors.len(), 3);
        let texts: Vec<&str> = pull
            .successors
            .iter()
            .map(|row| row.text.as_str())
            .collect();
        assert_eq!(
            texts,
            vec![
                "Ship the badge renderer",
                "Wire badges into the README",
                "Write badge docs",
            ]
        );
        for row in &pull.successors {
            assert_eq!(row.trigger, NOTE_PULL_TRIGGER);
            assert!(row.unblocked_by.is_empty());
            assert!(!row.inbox);
            assert_eq!(row.not_linked.as_deref(), Some("not_selected"));
            assert!(row.link.is_none());
            let offer = row.offer.as_ref().expect("offer");
            assert_eq!(offer.group, "note_pull");
            assert!(offer.project.is_none());
            assert!(!offer.selected);
        }
        assert_eq!(
            pull.successors
                .iter()
                .map(|row| row.offer.as_ref().expect("offer").n)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        // Write-free preview: staged bytes still equal the fixture inputs.
        assert_eq!(staged(&harness, "sase.md"), Some(PBA.to_string()));
        assert_eq!(staged(&harness, "day.md"), Some(FIX_DAY.to_string()));
    }

    // NP2: `-p all` links every offer; Ready becomes Next while Pending
    // and Next keep their symbols; missing IDs are minted; every pick is
    // stamped fresh and lands at a typed-or-implicit destination.
    #[test]
    fn np2_pick_all_links_statuses_mints_and_stamps() {
        let mut harness = vault_harness(
            &[("sase.md", PBA)],
            Some(FIX_DAY),
            SuccessorSelection::All,
        );
        let planned = plan(&mut harness).expect("NP2 plans");
        let pull = summary(&planned);
        assert_eq!(pull.successors.len(), 3);
        let symbols: Vec<char> = pull
            .successors
            .iter()
            .map(|row| row.status_symbol)
            .collect();
        assert_eq!(symbols, vec!['/', '*', '*']);
        for row in &pull.successors {
            assert!(row.link.is_some(), "picked row links");
            assert_eq!(row.not_linked, None);
            assert!(row.offer.as_ref().expect("offer").selected);
        }
        // The ID-less Ready row minted an ID and reports it.
        let ready = &pull.successors[2];
        assert!(!ready.block_id.is_empty());
        assert!(ready.link.as_ref().expect("link").block_id_created);
        assert!(
            !pull.successors[0]
                .link
                .as_ref()
                .expect("link")
                .block_id_created
        );
        // Source bytes: statuses flipped, IDs minted, fresh stamped.
        let staged_source = staged(&harness, "sase.md").expect("source staged");
        assert!(staged_source.contains("- [*] #task Write badge docs"));
        assert!(staged_source.contains("[fresh:: 2026-09-20]"));
        assert!(staged_source.contains(&format!("^{}", ready.block_id)));
        // Day bytes: all three links landed under FIX.
        let staged_day = staged(&harness, "day.md").expect("day staged");
        assert!(staged_day.contains("[[sase#^ship-renderer]]"));
        assert!(staged_day.contains("[[sase#^readme-badges]]"));
        assert!(staged_day.contains(&format!("[[sase#^{}]]", ready.block_id)));
    }

    // NP3: a live Task Link under today's open Pomodoros folds the row as
    // already planned.
    #[test]
    fn np3_live_link_folds_already_planned() {
        let day = format!("{FIX_DAY}  - [[sase#^readme-badges]]\n");
        let mut harness = vault_harness(
            &[("sase.md", PBA)],
            Some(&day),
            SuccessorSelection::None,
        );
        let planned = plan(&mut harness).expect("NP3 plans");
        let pull = summary(&planned);
        assert_eq!(pull.successors.len(), 2);
        assert_eq!(pull.folded.already_planned, 1);
        assert!(pull
            .successors
            .iter()
            .all(|row| row.text != "Wire badges into the README"));
    }

    // NP4: a struck link, or a link under a closed Pomodoro, is not live,
    // so the row stays offered.
    #[test]
    fn np4_struck_or_closed_links_stay_offered() {
        for day in [
            format!("{FIX_DAY}  - ~~[[sase#^readme-badges]]~~\n"),
            "## Pomodoros\n- [x] () \u{2014} FIX\n  - [[sase#^readme-badges]]\n- [ ] () \u{2014} WORK\n"
                .to_string(),
        ] {
            let mut harness = vault_harness(
                &[("sase.md", PBA)],
                Some(&day),
                SuccessorSelection::None,
            );
            let planned = plan(&mut harness).expect("NP4 plans");
            assert_eq!(summary(&planned).successors.len(), 3, "{day}");
        }
    }

    // NP5: fold precedence — blocked `?`, an open own prerequisite, and
    // `#prj_is_blocked` fold as blocked; future-scheduled folds as
    // scheduled; `#hide` folds as hidden; a scheduled date of today is
    // eligible.
    #[test]
    fn np5_fold_precedence_and_today_scheduled_offered() {
        let source = concat!(
            "- [?] #task Blocked row ^q\n",
            "- [ ] #task Needs proof [dependsOn:: open1] [id:: np] ^np\n",
            "- [ ] #task Opener [id:: open1] ^open1\n",
            "- [ ] #task Tagged #prj_is_blocked ^pb\n",
            "- [ ] #task Tomorrow [scheduled:: 2026-09-21] ^tm\n",
            "- [ ] #task Hidden #hide ^hi\n",
            "- [ ] #task Today [scheduled:: 2026-09-20] ^td\n",
            "- [ ] #task Plain ^pl\n",
        );
        let mut harness = vault_harness(
            &[("sase.md", source)],
            Some(FIX_DAY),
            SuccessorSelection::None,
        );
        let planned = plan(&mut harness).expect("NP5 plans");
        let pull = summary(&planned);
        assert_eq!(pull.folded.blocked, 3);
        assert_eq!(pull.folded.scheduled, 1);
        assert_eq!(pull.folded.hidden, 1);
        let texts: Vec<&str> = pull
            .successors
            .iter()
            .map(|row| row.text.as_str())
            .collect();
        assert!(texts.contains(&"Opener"));
        assert!(texts.contains(&"Today"));
        assert!(texts.contains(&"Plain"));
        assert!(!texts.contains(&"Needs proof"));
    }

    // NP6: the source note's own `^prj` is never offered or counted.
    #[test]
    fn np6_own_prj_never_offered_or_counted() {
        let source =
            concat!("- [ ] #task #prj Sase ^prj\n", "- [ ] #task Plain ^pl\n",);
        let mut harness = vault_harness(
            &[("sase.md", source)],
            Some(FIX_DAY),
            SuccessorSelection::None,
        );
        let planned = plan(&mut harness).expect("NP6 plans");
        let pull = summary(&planned);
        assert_eq!(pull.successors.len(), 1);
        assert_eq!(pull.successors[0].text, "Plain");
        assert_eq!(pull.folded.blocked, 0);
        assert_eq!(pull.folded.already_planned, 0);
        assert_eq!(pull.folded.scheduled, 0);
        assert_eq!(pull.folded.hidden, 0);
    }

    // NP7: only the named note's residents are offered; tasks living in
    // other files never appear.
    #[test]
    fn np7_only_source_residents_offered() {
        let mut harness = vault_harness(
            &[
                ("sase.md", "- [ ] #task Home row ^home\n"),
                ("other.md", "- [ ] #task Away row ^away\n"),
            ],
            Some(FIX_DAY),
            SuccessorSelection::None,
        );
        let planned = plan(&mut harness).expect("NP7 plans");
        let pull = summary(&planned);
        assert_eq!(pull.successors.len(), 1);
        assert_eq!(pull.successors[0].note_path, "sase.md");
    }

    // NP8: done, cancelled, and custom-status residents are ignored and
    // uncounted, so an all-closed note refuses with the no-open-tasks
    // copy.
    #[test]
    fn np8_closed_statuses_ignored_uncounted() {
        let source = concat!(
            "- [x] #task Done ^d\n",
            "- [-] #task Gone ^c\n",
            "- [>] #task Custom ^u\n",
        );
        let mut harness = vault_harness(
            &[("sase.md", source)],
            Some(FIX_DAY),
            SuccessorSelection::None,
        );
        let Err(error) = plan(&mut harness) else {
            panic!("NP8 refuses unexpectedly planned");
        };
        assert_eq!(error.code.as_deref(), Some("nothing_to_pull_in"));
        assert_eq!(
            error.message,
            "nothing to pull in from sase.md: it has no open tasks"
        );
        assert_eq!(staged(&harness, "sase.md"), Some(source.to_string()));
    }

    // NP9: `#ref` tasks, nested subtasks, and recurring tasks due today
    // are offered like any resident.
    #[test]
    fn np9_ref_nested_recurring_offered() {
        let source = concat!(
            "- [ ] #task #ref Read the paper [id:: rf] ^rf\n",
            "- [ ] #task Parent [id:: nest] ^nest\n",
            "  - [ ] #task Nested [id:: nested] ^nested\n",
            "- [ ] #task Water [repeat:: weekly] [id:: rec] ^rec\n",
        );
        let mut harness = vault_harness(
            &[("sase.md", source)],
            Some(FIX_DAY),
            SuccessorSelection::None,
        );
        let planned = plan(&mut harness).expect("NP9 plans");
        let texts: Vec<&str> = summary(&planned)
            .successors
            .iter()
            .map(|row| row.text.as_str())
            .collect();
        assert!(texts.contains(&"#ref Read the paper"), "{texts:?}");
        assert!(texts.contains(&"Nested"), "{texts:?}");
        assert!(texts.contains(&"Water"), "{texts:?}");
    }

    // NP10: every resident folded, or no open tasks at all, refuses with
    // `nothing_to_pull_in` and writes nothing.
    #[test]
    fn np10_empty_refusals_carry_fold_and_write_nothing() {
        let mut harness = vault_harness(
            &[(
                "sase.md",
                "- [?] #task Blocked ^q\n- [ ] #task Hidden #hide ^hi\n",
            )],
            Some(FIX_DAY),
            SuccessorSelection::All,
        );
        let Err(error) = plan(&mut harness) else {
            panic!("NP10 folded refuses unexpectedly planned");
        };
        assert_eq!(error.code.as_deref(), Some("nothing_to_pull_in"));
        assert_eq!(
            error.message,
            "nothing to pull in from sase.md: 1 blocked · 1 hidden"
        );
        let payload = error
            .extra
            .as_ref()
            .and_then(|extra| extra.get("note_pull"))
            .expect("note_pull payload");
        assert_eq!(payload["note"], "sase.md");
        assert_eq!(payload["route"], "sase");
        assert_eq!(payload["folded"]["blocked"], 1);
        assert_eq!(payload["folded"]["hidden"], 1);
        assert_eq!(
            staged(&harness, "sase.md"),
            Some(
                "- [?] #task Blocked ^q\n- [ ] #task Hidden #hide ^hi\n"
                    .to_string()
            )
        );
        assert_eq!(staged(&harness, "day.md"), Some(FIX_DAY.to_string()));
    }

    // NP11: a blocked project names its `^prj` reason in the empty-pull
    // message instead of the plain fold line.
    #[test]
    fn np11_blocked_project_names_prj_reason() {
        let mut harness = vault_harness(
            &[
                (
                    "sase.md",
                    concat!(
                        "- [?] #task #prj Sase [dependsOn:: open1] ^prj\n",
                        "- [ ] #task Resident #prj_is_blocked ^r\n",
                    ),
                ),
                ("other.md", "- [ ] #task Opener [id:: open1] ^open1\n"),
            ],
            Some(FIX_DAY),
            SuccessorSelection::All,
        );
        let Err(error) = plan(&mut harness) else {
            panic!("NP11 refuses unexpectedly planned");
        };
        assert_eq!(error.code.as_deref(), Some("nothing_to_pull_in"));
        assert_eq!(
            error.message,
            "nothing to pull in from sase.md: the project is blocked \
             (its ^prj waits on 1 task)"
        );
    }

    #[test]
    fn np11_scheduled_project_names_date_reason() {
        let mut harness = vault_harness(
            &[(
                "sase.md",
                concat!(
                    "- [?] #task #prj Sase [scheduled:: 2026-09-21] ^prj\n",
                    "- [ ] #task Resident #prj_is_blocked ^r\n",
                ),
            )],
            Some(FIX_DAY),
            SuccessorSelection::All,
        );
        let Err(error) = plan(&mut harness) else {
            panic!("NP11 scheduled refuses unexpectedly planned");
        };
        assert_eq!(error.code.as_deref(), Some("nothing_to_pull_in"));
        assert_eq!(
            error.message,
            "nothing to pull in from sase.md: the project is blocked \
             (is scheduled for 2026-09-21)"
        );
    }

    // NP12: a missing source note refuses `pull_note_missing` and writes
    // nothing.
    #[test]
    fn np12_missing_source_refuses_pull_note_missing() {
        let mut harness =
            vault_harness(&[], Some(FIX_DAY), SuccessorSelection::None);
        let Err(error) = plan(&mut harness) else {
            panic!("NP12 refuses unexpectedly planned");
        };
        assert_eq!(error.code.as_deref(), Some("pull_note_missing"));
        assert_eq!(
            error.message,
            "@sase pulls ready tasks from sase.md, which does not exist: \
             add `- <task>` bullets below it to start a list there, or run \
             'bob capture-targets' to list routable notes"
        );
        assert_eq!(staged(&harness, "sase.md"), None);
        assert_eq!(staged(&harness, "day.md"), None);
    }

    // NP15: `-p none` succeeds, reports every offer unselected, mints no
    // IDs, and writes nothing.
    #[test]
    fn np15_none_selection_writes_nothing_and_mints_nothing() {
        let source =
            concat!("- [*] #task Has id ^b\n", "- [ ] #task No id yet\n",);
        let mut harness = vault_harness(
            &[("sase.md", source)],
            Some(FIX_DAY),
            SuccessorSelection::None,
        );
        let planned = plan(&mut harness).expect("NP15 plans");
        let pull = summary(&planned);
        assert_eq!(pull.successors.len(), 2);
        assert!(pull.successors.iter().all(|row| row.link.is_none()
            && row.not_linked.as_deref() == Some("not_selected")));
        let idless = pull
            .successors
            .iter()
            .find(|row| row.text == "No id yet")
            .expect("id-less offer");
        assert_eq!(idless.block_id, "");
        assert_eq!(staged(&harness, "sase.md"), Some(source.to_string()));
        assert_eq!(staged(&harness, "day.md"), Some(FIX_DAY.to_string()));
    }

    // NP21: destinations use the normal typed rules without a predecessor;
    // without theme config a typeless pick lands implicit with fallback.
    // A placeholder is written only when a picked link lands there: with
    // no picks the day is untouched, and a subset pick links only the
    // picked row even when an unselected same-shape row precedes it.
    #[test]
    fn np21_subset_pick_links_only_picked_row() {
        let source = concat!(
            "- [ ] #task First typeless ^first\n",
            "- [ ] #task Second typeless ^second\n",
        );
        let mut harness = vault_harness(
            &[("sase.md", source)],
            Some(FIX_DAY),
            SuccessorSelection::Keys(
                ["sase.md#^second".to_string()].into_iter().collect(),
            ),
        );
        let planned = plan(&mut harness).expect("NP21 plans");
        let pull = summary(&planned);
        assert_eq!(pull.successors.len(), 2);
        assert!(!pull.successors[0].offer.as_ref().expect("offer").selected);
        assert!(pull.successors[1].offer.as_ref().expect("offer").selected);
        assert!(pull.successors[1].link.is_some());
        assert!(
            pull.successors[1]
                .offer
                .as_ref()
                .expect("offer")
                .destination
                .fallback
        );
        let staged_day = staged(&harness, "day.md").expect("day staged");
        assert!(staged_day.contains("[[sase#^second]]"));
        assert!(!staged_day.contains("[[sase#^first]]"));
        let staged_source = staged(&harness, "sase.md").expect("source staged");
        assert!(staged_source.contains("- [*] #task Second typeless"));
        assert!(staged_source.contains("- [ ] #task First typeless"));
    }

    // NP22: when prerequisite proof is unavailable, a dependency-bearing
    // candidate folds as blocked instead of being offered.
    #[test]
    fn np22_unavailable_proof_folds_candidate_blocked() {
        let proof = SnapshotProof {
            files: Vec::new(),
            states: BTreeMap::new(),
        };
        let task = crate::native::task_status_hooks::TaskLine {
            line_index: 0,
            status: ' ',
            status_byte_offset: 0,
            block_id: Some("np".to_string()),
            task_id: Some("np".to_string()),
            depends_on: vec!["open1".to_string()],
            scheduled: None,
            status_type: crate::native::task_status_hooks::TaskStatusType::Todo,
            status_recognized: true,
            description: "Needs proof".to_string(),
        };
        assert!(proof.is_blocked(Path::new("sase.md"), &task));
    }

    // NP23: no daily note, or a daily note without an eligible Pomodoro
    // section, returns the usual error and writes nothing.
    #[test]
    fn np23_missing_day_uses_existing_error() {
        let mut harness =
            vault_harness(&[("sase.md", PBA)], None, SuccessorSelection::All);
        let Err(error) = plan(&mut harness) else {
            panic!("NP23 missing day unexpectedly planned");
        };
        assert!(error.message.contains("Bob daily note does not exist"));
        assert_eq!(staged(&harness, "sase.md"), Some(PBA.to_string()));
    }

    #[test]
    fn np23_day_without_pomodoros_uses_existing_error() {
        let mut harness = vault_harness(
            &[("sase.md", PBA)],
            Some("# Day\nno pomodoros here\n"),
            SuccessorSelection::All,
        );
        let Err(error) = plan(&mut harness) else {
            panic!("NP23 no section unexpectedly planned");
        };
        assert!(!error.message.is_empty());
        assert_eq!(staged(&harness, "sase.md"), Some(PBA.to_string()));
        assert_eq!(
            staged(&harness, "day.md"),
            Some("# Day\nno pomodoros here\n".to_string())
        );
    }
}

//! `move_choice` report for `- ^^` drafts (`cli-report` phase).
//!
//! Builds the batch-level `move_choice` object from the staged parent,
//! parses `-m/--move-in` answers, and enforces the write-free refusal
//! order from the move_choice contract.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use chrono::NaiveDate;
use serde::Serialize;

use crate::native::capture_block_ids;
use crate::native::note_tasks;
use crate::native::task_move;

// ---------------------------------------------------------------------------
// `-m/--move-in` picks
// ---------------------------------------------------------------------------

/// One parsed `-m` token: move (`^`) or move-and-link (`:`) one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MoveInPick {
    pub(crate) sigil: char,
    pub(crate) selector: String,
    pub(crate) is_number: bool,
}

/// Parsed `-m/--move-in` value. `None` on the request means no flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MoveInPicks {
    All,
    None,
    Tokens(Vec<MoveInPick>),
}

/// Parse one `--move-in` comma list value.
pub(super) fn parse_move_in_picks(
    raw: &str,
) -> Result<MoveInPicks, super::output::CaptureError> {
    use super::output::CaptureError;
    let mut picks = Vec::new();
    let mut saw_all = false;
    let mut saw_none = false;
    for token in raw.split(',') {
        let token = token.trim();
        if token.is_empty() {
            return Err(CaptureError::usage(
                "--move-in has an empty token; use all, none, or a comma list of row numbers and keys",
            ));
        }
        // Optional leading sigil.
        let (sigil, rest) = match token.strip_prefix(':') {
            Some(rest) => (':', rest),
            None => match token.strip_prefix('^') {
                Some(rest) => ('^', rest),
                None => ('^', token),
            },
        };
        let has_prefix = token.starts_with(':') || token.starts_with('^');
        if rest == "all" || rest == "none" {
            if has_prefix {
                return Err(CaptureError::usage(format!(
                    "--move-in pick `{token}` is not a valid number or key"
                )));
            }
            if rest == "all" {
                saw_all = true;
            } else {
                saw_none = true;
            }
            continue;
        }
        if token == "all" {
            saw_all = true;
            continue;
        }
        if token == "none" {
            saw_none = true;
            continue;
        }
        // After stripping sigil, `all`/`none` with prefix already rejected.
        // Numbers are all digits; keys contain `#^` or `@`.
        if rest.bytes().all(|b| b.is_ascii_digit()) {
            if rest.is_empty() {
                return Err(CaptureError::usage(
                    "--move-in has an empty token; use all, none, or a comma list of row numbers and keys",
                ));
            }
            picks.push(MoveInPick {
                sigil,
                selector: rest.to_string(),
                is_number: true,
            });
        } else if rest.contains("#^") || rest.contains('@') {
            picks.push(MoveInPick {
                sigil,
                selector: rest.to_string(),
                is_number: false,
            });
        } else if rest == "all" || rest == "none" {
            // Prefix form already rejected above; bare form handled.
            if rest == "all" {
                saw_all = true;
            } else {
                saw_none = true;
            }
        } else {
            return Err(CaptureError::usage(format!(
                "--move-in pick `{token}` is not a valid number or key"
            )));
        }
    }
    if saw_all || saw_none {
        // `all`/`none` mixed handling is a conflict, but a lone `all` or
        // lone `none` across repeated flags also needs representation.
        // Encode as All/None only when no other picks; otherwise keep
        // tokens plus markers for the conflict check in resolution.
        if !picks.is_empty() {
            // Represent conflict via special picks; resolution reports it.
            // Push sentinel picks so `plan` can distinguish.
            if saw_all {
                picks.push(MoveInPick {
                    sigil: '^',
                    selector: "__all__".to_string(),
                    is_number: false,
                });
            }
            if saw_none {
                picks.push(MoveInPick {
                    sigil: '^',
                    selector: "__none__".to_string(),
                    is_number: false,
                });
            }
            // Also remember bare all/none via extra flags encoded in
            // selector sentinels; resolution checks them first.
            return Ok(MoveInPicks::Tokens(picks));
        }
        if saw_all && saw_none {
            // Both alone across flags: conflict.
            return Ok(MoveInPicks::Tokens(vec![
                MoveInPick {
                    sigil: '^',
                    selector: "__all__".to_string(),
                    is_number: false,
                },
                MoveInPick {
                    sigil: '^',
                    selector: "__none__".to_string(),
                    is_number: false,
                },
            ]));
        }
        if saw_all {
            return Ok(MoveInPicks::All);
        }
        return Ok(MoveInPicks::None);
    }
    Ok(MoveInPicks::Tokens(picks))
}

/// Merge repeated `--move-in` values, unioning their picks.
pub(super) fn merge_move_in_picks(
    values: Vec<MoveInPicks>,
) -> Option<MoveInPicks> {
    if values.is_empty() {
        return None;
    }
    let mut all = false;
    let mut none = false;
    let mut tokens: Vec<MoveInPick> = Vec::new();
    for value in values {
        match value {
            MoveInPicks::All => all = true,
            MoveInPicks::None => none = true,
            MoveInPicks::Tokens(mut picks) => tokens.append(&mut picks),
        }
    }
    // If any All/None appeared alongside tokens, keep sentinels for
    // conflict reporting.
    if all {
        if none || !tokens.is_empty() {
            tokens.push(MoveInPick {
                sigil: '^',
                selector: "__all__".to_string(),
                is_number: false,
            });
            if none {
                tokens.push(MoveInPick {
                    sigil: '^',
                    selector: "__none__".to_string(),
                    is_number: false,
                });
            }
            return Some(MoveInPicks::Tokens(tokens));
        }
        return Some(MoveInPicks::All);
    }
    if none {
        if !tokens.is_empty() {
            tokens.push(MoveInPick {
                sigil: '^',
                selector: "__none__".to_string(),
                is_number: false,
            });
            return Some(MoveInPicks::Tokens(tokens));
        }
        return Some(MoveInPicks::None);
    }
    Some(MoveInPicks::Tokens(tokens))
}

// ---------------------------------------------------------------------------
// Report JSON
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MoveChoiceReport {
    pub(crate) supported: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) slots: Option<Vec<MoveChoiceSlot>>,
}

impl MoveChoiceReport {
    pub(crate) fn empty() -> Self {
        Self {
            supported: true,
            slots: None,
        }
    }

    pub(crate) fn with_slots(slots: Vec<MoveChoiceSlot>) -> Self {
        Self {
            supported: true,
            slots: Some(slots),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MoveChoiceSlot {
    pub(crate) item: usize,
    pub(crate) item_line: usize,
    pub(crate) draft_range: (usize, usize),
    pub(crate) parent: String,
    pub(crate) destination: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) pomodoro_name: Option<String>,
    pub(crate) name_needs_link: bool,
    pub(crate) open_count: usize,
    pub(crate) answered: bool,
    pub(crate) moved_count: usize,
    pub(crate) linked_count: usize,
    pub(crate) copy: MoveChoiceCopy,
    pub(crate) rows: Vec<MoveChoiceRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MoveChoiceCopy {
    pub(crate) title: String,
    pub(crate) subtitle: String,
    pub(crate) slot: String,
    pub(crate) leave: String,
    pub(crate) ghost: String,
    pub(crate) band: String,
    pub(crate) then: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MoveChoiceRow {
    pub(crate) key: String,
    pub(crate) n: usize,
    pub(crate) line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) section: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) block_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) mint: Option<String>,
    pub(crate) text: String,
    pub(crate) status_symbol: char,
    pub(crate) status_name: String,
    pub(crate) hidden: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) scheduled: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) pomodoro: Option<MoveChoicePomodoro>,
    pub(crate) choices: Vec<MoveChoiceOption>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) locked: Option<MoveChoiceLock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) disabled_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) answer: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MoveChoiceOption {
    pub(crate) sigil: String,
    pub(crate) consequence: String,
    pub(crate) adds_link: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MoveChoiceLock {
    pub(crate) sigil: String,
    pub(crate) item_line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MoveChoicePomodoro {
    pub(crate) line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
}

// ---------------------------------------------------------------------------
// Builder helpers
// ---------------------------------------------------------------------------

/// Draft-wide locks: `(parent_label, block_id) -> (sigil, item_line)`.
/// Computed from all parsed items before planning.
pub(super) fn collect_move_locks(
    raw_text: &str,
    parsed_items: &[crate::native::capture_language::ParsedCaptureItem],
) -> HashMap<(String, String), (char, usize)> {
    let mut locks: HashMap<(String, String), (char, usize)> = HashMap::new();
    for item in parsed_items {
        let route = item.parsed.route.clone().unwrap_or_default();
        if route.is_empty() {
            continue;
        }
        // Only sealed project-note items claim locks.
        let is_project_note = matches!(
            item.parsed.kind,
            crate::native::capture_language::CaptureKind::ProjectNote { .. }
        );
        if !is_project_note {
            continue;
        }
        let parent = format!("{route}.md");
        for sub in &item.parsed.sub_bullets {
            if let Some(movement) = sub.move_task.as_ref()
                && let Some(block_id) = movement.block_id.as_deref()
            {
                let line = draft_line_number(raw_text, movement.range.0);
                locks
                    .entry((parent.clone(), block_id.to_string()))
                    .or_insert((movement.sigil, line));
            }
        }
    }
    locks
}

fn draft_line_number(text: &str, offset: usize) -> usize {
    text.get(..offset.min(text.len()))
        .map(|prefix| prefix.bytes().filter(|byte| *byte == b'\n').count() + 1)
        .unwrap_or(1)
}

fn display_move_date(date: &str) -> String {
    // `YYYY-MM-DD` -> `Mon D` like `Nov 6`.
    if let Ok(parsed) = NaiveDate::parse_from_str(date, "%Y-%m-%d") {
        let month = match parsed.format("%b").to_string().as_str() {
            "Jan" => "Jan",
            "Feb" => "Feb",
            "Mar" => "Mar",
            "Apr" => "Apr",
            "May" => "May",
            "Jun" => "Jun",
            "Jul" => "Jul",
            "Aug" => "Aug",
            "Sep" => "Sep",
            "Oct" => "Oct",
            "Nov" => "Nov",
            "Dec" => "Dec",
            _ => return date.to_string(),
        };
        return format!("{} {}", month, parsed.format("%-d"));
    }
    date.to_string()
}

/// Build one slot from the staged parent. Returns `None` when the item
/// has no `- ^^`.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_slot(
    request_bob_dir: &Path,
    raw_text: &str,
    parsed: &crate::native::capture_language::ParsedCaptureText,
    parsed_index: usize,
    planner: &mut super::batch::CaptureBatchPlanner,
    today: NaiveDate,
    locks: &HashMap<(String, String), (char, usize)>,
    dependency_ctx: &mut super::dependencies::DependencyContext,
) -> Result<Option<MoveChoiceSlot>, super::output::CaptureError> {
    use super::output::CaptureError;
    use crate::native::capture_language::CaptureKind;

    let choice_pos = parsed
        .sub_bullets
        .iter()
        .position(|b| b.move_choice.is_some());
    let Some(choice_idx) = choice_pos else {
        return Ok(None);
    };
    let choice = parsed.sub_bullets[choice_idx]
        .move_choice
        .as_ref()
        .expect("choice position");
    let draft_range = choice.range;

    let CaptureKind::ProjectNote {
        block_id,
        pomodoro_name,
        ..
    } = &parsed.kind
    else {
        return Ok(None);
    };
    let route = parsed.route.clone().ok_or_else(|| {
        CaptureError::io("project-note move invariant failed: route is missing")
    })?;
    let parent = format!("{route}.md");
    let destination =
        crate::native::capture_project_note::project_note_basename(
            &route, block_id,
        );
    let item = parsed_index + 1;
    let item_line = draft_line_number(raw_text, draft_range.0);

    let source_path = request_bob_dir.join(&parent);
    let Some(contents) = planner.current_contents(&source_path)? else {
        return Err(CaptureError::io(format!(
            "cannot create a project note under {parent}: note does not exist (run 'bob capture-targets' to list routable notes)"
        )));
    };
    // Original text for locked rows removed by this batch's explicit moves.
    // `loaded_texts` returns (original, current) when the file was touched.
    let original_contents: Option<String> =
        planner.loaded_texts(&source_path).map(|(o, _)| o);

    // Rows from movable roots, excluding ^prj and the template placeholder.
    // Use staged contents, but re-add locked rows missing from staged.
    let mut roots = task_move::movable_roots(Path::new(&parent), &contents);
    if let Some(original) = original_contents.as_deref() {
        let original_roots =
            task_move::movable_roots(Path::new(&parent), original);
        let staged_ids: HashSet<Option<String>> =
            roots.iter().map(|r| r.block_id.clone()).collect();
        // Original line order for stable document ordering when staged
        // lines shifted after explicit moves.
        let orig_line_by_id: HashMap<String, usize> = original_roots
            .iter()
            .filter_map(|r| r.block_id.clone().map(|id| (id, r.line)))
            .collect();
        for orig in original_roots {
            // Only re-add ID-bearing rows claimed by this draft (locks).
            if let Some(id) = orig.block_id.clone() {
                if staged_ids.contains(&Some(id.clone())) {
                    continue;
                }
                if locks.contains_key(&(parent.clone(), id.clone())) {
                    roots.push(orig);
                }
            }
        }
        // Document order by original line when known, else staged line.
        roots.sort_by_key(|r| {
            r.block_id
                .as_deref()
                .and_then(|id| orig_line_by_id.get(id).copied())
                .unwrap_or(r.line)
        });
    }
    let settings = note_tasks::read_settings(request_bob_dir);
    // Scan staged for section/hidden, but locked rows missing from staged
    // need original scan for their metadata. Build a combined lookup.
    let scan = note_tasks::scan(&contents, &settings);
    let lines: Vec<&str> = contents.lines().collect();
    let original_scan = original_contents
        .as_deref()
        .map(|o| note_tasks::scan(o, &settings));
    let original_lines: Option<Vec<&str>> =
        original_contents.as_deref().map(|o| o.lines().collect());

    // Destination future-date blocking.
    // The parsed scheduled date (YYYY-MM-DD) if any.
    let scheduled_date: Option<String> = parsed
        .scheduled_offset
        .map(|off| super::plan::scheduled_date_string(today, off))
        .transpose()?;
    let future_project_date = scheduled_date
        .as_deref()
        .filter(|d| *d > super::plan::date_string(today).as_str())
        .map(str::to_string);

    // Used IDs for mints: parent IDs + new note authored IDs + prj + other mints.
    let mut used: HashSet<String> = HashSet::new();
    for task in scan.tasks() {
        if let Some(id) = task.block_id.as_deref() {
            used.insert(id.to_string());
        }
    }
    for sub in &parsed.sub_bullets {
        if let Some(task) = sub.task_id.as_ref() {
            used.insert(task.block_id.clone());
        }
        if let Some(movement) = sub.move_task.as_ref()
            && let Some(id) = movement.block_id.as_deref()
        {
            used.insert(id.to_string());
        }
    }
    used.insert("prj".to_string());
    let global_filter = settings.global_filter.clone();

    // Pomodoro ledger for chips.
    let day_file = crate::native::pomodoro::day_file_for(request_bob_dir);
    let day_contents = planner.current_contents(&day_file).ok().flatten();
    let ledger = day_contents
        .as_deref()
        .map(crate::native::capture_active_tasks::ledger_from_contents);

    // Filter roots: exclude ^prj and placeholder.
    let mut filtered: Vec<(usize, task_move::MovableRoot)> = Vec::new();
    for root in roots {
        if root.block_id.as_deref() == Some("prj") {
            continue;
        }
        if root.title
            == crate::native::capture_project_note::PLACEHOLDER_TASK_BODY
        {
            continue;
        }
        // Closed/nested already excluded by movable_roots.
        filtered.push((root.line, root));
    }
    // Document order is already line order; keep it.

    // Precompute mints for ID-less rows.
    let mut used_for_mint: HashSet<String> = used.clone();
    // Reserve: also avoid collisions across mints.
    let mut mints: HashMap<usize, String> = HashMap::new();
    for (idx, (_line, root)) in filtered.iter().enumerate() {
        if root.block_id.is_none() {
            let used_ref: HashSet<&str> =
                used_for_mint.iter().map(String::as_str).collect();
            let mint = capture_block_ids::mint_block_id(&root.title, &used_ref);
            used_for_mint.insert(mint.clone());
            mints.insert(idx, mint);
        }
    }

    // name_needs_link: Create form with #name and no other : link.
    let form = match &parsed.kind {
        CaptureKind::ProjectNote { form, .. } => *form,
        _ => crate::native::capture_language::ProjectNoteForm::Create,
    };
    let has_colon = parsed.sub_bullets.iter().any(|b| {
        b.task_id.as_ref().is_some_and(|t| t.link)
            || b.move_task.as_ref().is_some_and(|m| m.sigil == ':')
    });
    let name_needs_link = matches!(
        form,
        crate::native::capture_language::ProjectNoteForm::Create
    ) && pomodoro_name.is_some()
        && !has_colon;

    // Copy slot neighbours: nearest authored task texts around ^^.
    let (before, after) = slot_neighbours(parsed, &contents, request_bob_dir);
    let slot_copy = if let (Some(b), Some(a)) = (before, after) {
        format!("Lands between “{b}” and “{a}”, in {parent} order")
    } else if let Some(b) =
        slot_neighbours(parsed, &contents, request_bob_dir).0
    {
        format!("Lands after “{b}”, in {parent} order")
    } else if let Some(a) =
        slot_neighbours(parsed, &contents, request_bob_dir).1
    {
        format!("Lands before “{a}”, in {parent} order")
    } else {
        format!("Lands first, in {parent} order")
    };

    let open_count = filtered.len();
    let dest_label = destination.clone();

    let mut rows: Vec<MoveChoiceRow> = Vec::new();
    for (doc_idx, (_line_no, root)) in filtered.iter().enumerate() {
        let n = doc_idx + 1;
        let task = scan.task_at(root.line.saturating_sub(1)).or_else(|| {
            original_scan
                .as_ref()
                .and_then(|s| s.task_at(root.line.saturating_sub(1)))
        });
        let section = task.and_then(|t| t.section.clone());
        let status_name =
            task.map(|t| t.status_name.clone()).unwrap_or_else(|| {
                super::task_complete::status_name_for(
                    request_bob_dir,
                    root.status,
                )
            });
        let raw_line = lines
            .get(root.line.saturating_sub(1))
            .copied()
            .or_else(|| {
                original_lines
                    .as_ref()
                    .and_then(|v| v.get(root.line.saturating_sub(1)).copied())
            })
            .unwrap_or_default();
        let hidden =
            crate::native::capture_dependency_tasks::has_hide_tag(raw_line);
        let scheduled_field =
            crate::native::task_fields::inline_fields(raw_line, "scheduled")
                .into_iter()
                .find_map(|f| {
                    crate::native::task_fields::parse_strict_calendar_date(
                        &f.value,
                    )
                    .map(|_| f.value)
                });
        let pomodoro_chip: Option<MoveChoicePomodoro> =
            root.block_id.as_deref().and_then(|id| {
                ledger.as_ref().and_then(|ledger| {
                    ledger.owners.get(&(route.clone(), id.to_string())).map(
                        |entry| MoveChoicePomodoro {
                            line: entry.line,
                            name: entry.name.clone(),
                        },
                    )
                })
            });
        let key = if let Some(id) = root.block_id.as_deref() {
            format!("{parent}#^{id}")
        } else {
            // Stable key via digest ordinal among equal descriptions.
            let cleaned = note_tasks::clean_description(
                &root.title,
                &global_filter,
                None,
            );
            // Ordinal among filtered ID-less rows with same cleaned text.
            let mut ordinal = 1;
            for (prev_idx, (_, prev)) in filtered.iter().enumerate() {
                if prev_idx >= doc_idx {
                    break;
                }
                if prev.block_id.is_none()
                    && note_tasks::clean_description(
                        &prev.title,
                        &global_filter,
                        None,
                    ) == cleaned
                {
                    ordinal += 1;
                }
            }
            note_tasks::stable_task_key(
                Path::new(&parent),
                None,
                &root.title,
                &global_filter,
                ordinal,
            )
        };
        let mint = mints.get(&doc_idx).cloned();

        // Locks.
        let locked: Option<MoveChoiceLock> =
            root.block_id.as_deref().and_then(|id| {
                locks.get(&(parent.clone(), id.to_string())).map(
                    |(sigil, line)| MoveChoiceLock {
                        sigil: sigil.to_string(),
                        item_line: *line,
                    },
                )
            });

        // Disabled: source reason or future project date.
        let source_disabled = root.disabled_reason.clone();
        let project_disabled: Option<String> =
            future_project_date.as_deref().and_then(|date| {
                if root.status != '?' {
                    Some(format!(
                        "the project starts {}, so only Blocked tasks can move in",
                        display_move_date(date)
                    ))
                } else {
                    None
                }
            });
        let disabled_reason = project_disabled
            .or(source_disabled)
            .filter(|_| locked.is_none());
        // If locked, disabled is None (locked wins).
        let disabled_reason = if locked.is_some() {
            None
        } else {
            disabled_reason
        };

        // Choices via consequence builder.
        let choices: Vec<MoveChoiceOption> =
            if locked.is_some() || disabled_reason.is_some() {
                Vec::new()
            } else {
                let mut opts = Vec::new();
                for sigil in ['^', ':'] {
                    let guard = guard_for_row(
                        request_bob_dir,
                        planner,
                        dependency_ctx,
                        &parent,
                        root.block_id.as_deref(),
                        root.status,
                        future_project_date.as_deref(),
                        today,
                    );
                    let today_link =
                        pomodoro_chip.as_ref().and_then(|p| p.name.clone());
                    let prediction = super::move_consequence::predict(
                        &super::move_consequence::PredictionInput {
                            sigil,
                            status: root.status,
                            destination: dest_label.clone(),
                            pomodoro_name: pomodoro_name.clone(),
                            today_link,
                            own_scheduled: scheduled_field.clone(),
                            project_starts: future_project_date
                                .as_deref()
                                .map(display_move_date),
                            guard,
                            id_to_add: mint.clone(),
                        },
                    );
                    opts.push(MoveChoiceOption {
                        sigil: sigil.to_string(),
                        consequence: prediction.consequence,
                        adds_link: prediction.adds_link,
                    });
                }
                opts
            };

        rows.push(MoveChoiceRow {
            key,
            n,
            line: root.line,
            section,
            block_id: root.block_id.clone(),
            mint,
            text: root.title.clone(),
            status_symbol: root.status,
            status_name,
            hidden,
            scheduled: scheduled_field,
            pomodoro: pomodoro_chip,
            choices,
            locked,
            disabled_reason,
            answer: None,
        });
    }

    let ghost = format!("choose from {parent}'s {open_count} open tasks on ↩");
    let band = format!("Asks about {parent}'s {open_count} open tasks on ↩");
    let copy = MoveChoiceCopy {
        title: format!("Move into {destination}"),
        subtitle: format!("from {parent} · {open_count} open"),
        slot: slot_copy,
        leave: format!("Stays in {parent}"),
        ghost,
        band,
        then: None,
    };

    Ok(Some(MoveChoiceSlot {
        item,
        item_line,
        draft_range,
        parent,
        destination,
        pomodoro_name: pomodoro_name.clone(),
        name_needs_link,
        open_count,
        answered: false,
        moved_count: 0,
        linked_count: 0,
        copy,
        rows,
    }))
}

fn slot_neighbours(
    parsed: &crate::native::capture_language::ParsedCaptureText,
    _parent_contents: &str,
    _bob_dir: &Path,
) -> (Option<String>, Option<String>) {
    let choice_idx = parsed
        .sub_bullets
        .iter()
        .position(|b| b.move_choice.is_some())
        .unwrap_or(0);
    let before = parsed.sub_bullets[..choice_idx].iter().rev().find_map(|b| {
        if !b.body.is_empty() {
            Some(b.body.clone())
        } else if let Some(m) = b.move_task.as_ref() {
            m.block_id.clone()
        } else {
            None
        }
    });
    let after = parsed.sub_bullets[choice_idx + 1..].iter().find_map(|b| {
        if !b.body.is_empty() {
            Some(b.body.clone())
        } else if let Some(m) = b.move_task.as_ref() {
            m.block_id.clone()
        } else {
            None
        }
    });
    (before, after)
}

fn guard_for_row(
    bob_dir: &Path,
    planner: &mut super::batch::CaptureBatchPlanner,
    dependency_ctx: &mut super::dependencies::DependencyContext,
    parent: &str,
    block_id: Option<&str>,
    status: char,
    future_project: Option<&str>,
    today: NaiveDate,
) -> super::move_consequence::LinkGuard {
    use super::move_consequence::LinkGuard;
    // Future-dated destination: `:` stays unlinked until start.
    if future_project.is_some() {
        // The consequence builder handles project_starts for `:`; the
        // guard stays Link so `^` keeps its Blocked copy.
        return LinkGuard::Link;
    }
    let Some(block_id) = block_id else {
        // ID-less rows: status-only, like capture-complete.
        if status == '?' {
            return LinkGuard::StatusOnly;
        }
        return LinkGuard::Link;
    };
    if status != '?' {
        // Fast path: no local inputs means Link without a graph.
        // Assess via the link guard; fall back to Link on error.
        let target_rel = Path::new(parent);
        match super::link_guard::assess_link_target(
            dependency_ctx,
            planner,
            bob_dir,
            target_rel,
            block_id,
            today,
        ) {
            Ok(super::link_guard::LinkGuardVerdict::Link) => LinkGuard::Link,
            Ok(super::link_guard::LinkGuardVerdict::PullForward) => {
                // Need the date: read scheduled field from parent line.
                LinkGuard::Link
            }
            Ok(super::link_guard::LinkGuardVerdict::DropDependencies(plan)) => {
                LinkGuard::DropPrerequisites {
                    count: plan.open_prerequisites.len(),
                    pulled_forward: None,
                }
            }
            Ok(super::link_guard::LinkGuardVerdict::Consume(plan)) => {
                let reason = plan
                    .reasons
                    .first()
                    .map(|r| r.label.clone())
                    .unwrap_or_else(|| "blocked".to_string());
                LinkGuard::Consume { reason }
            }
            Err(_) => LinkGuard::Link,
        }
    } else {
        // Blocked: assess for real.
        let target_rel = Path::new(parent);
        match super::link_guard::assess_link_target(
            dependency_ctx,
            planner,
            bob_dir,
            target_rel,
            block_id,
            today,
        ) {
            Ok(super::link_guard::LinkGuardVerdict::Link) => LinkGuard::Link,
            Ok(super::link_guard::LinkGuardVerdict::PullForward) => {
                // Pull-forward date from the task's own scheduled field.
                LinkGuard::Link
            }
            Ok(super::link_guard::LinkGuardVerdict::DropDependencies(plan)) => {
                LinkGuard::DropPrerequisites {
                    count: plan.open_prerequisites.len(),
                    pulled_forward: None,
                }
            }
            Ok(super::link_guard::LinkGuardVerdict::Consume(plan)) => {
                let reason = plan
                    .reasons
                    .first()
                    .map(|r| r.label.clone())
                    .unwrap_or_else(|| "blocked".to_string());
                LinkGuard::Consume { reason }
            }
            Err(_) => LinkGuard::StatusOnly,
        }
    }
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// Resolve `-m` answers against rebuilt rows. Returns the per-row
/// `(sigil)` answers in parent order. Errors use §3.7 codes.
pub(super) fn resolve_picks(
    slot: &MoveChoiceSlot,
    picks: Option<&MoveInPicks>,
    is_dry_run: bool,
) -> Result<Vec<Option<char>>, super::output::CaptureError> {
    use super::output::CaptureError;
    let report = MoveChoiceReport::with_slots(vec![slot.clone()]);
    let report_value =
        serde_json::to_value(&report).expect("serialize move_choice");

    let extra_with_report = || {
        let mut extra = serde_json::Map::new();
        extra.insert("move_choice".to_string(), report_value.clone());
        extra
    };

    let Some(picks) = picks else {
        // No `-m`.
        if is_dry_run {
            return Ok(vec![None; slot.rows.len()]);
        }
        let extra = extra_with_report();
        // Include fresh rows? The report already has them.
        return Err(CaptureError {
            kind: super::output::CaptureErrorKind::Io,
            message: format!(
                "`- ^^` on line {} asks which of {}'s {} open tasks move in; answer with -m 1,3 (move), -m :2 (move & link), or -m none",
                slot.item_line,
                slot.parent,
                slot.open_count
            ),
            code: Some("move_choice_required".to_string()),
            extra: Some(extra),
            human_unblocked: Vec::new(),
            human_keep_moving_folds: Vec::new(),
            human_relation_groups: Vec::new(),
            human_relation_held: Vec::new(),
            human_relation_check: None,
            human_note_pull_rows: Vec::new(),
            human_note_pull_groups: Vec::new(),
            typed: None,
        });
    };

    // `none` / `all` handling.
    match picks {
        MoveInPicks::None => Ok(vec![None; slot.rows.len()]),
        MoveInPicks::All => {
            // Every enabled, unlocked row as plain move.
            let mut answers = vec![None; slot.rows.len()];
            for (idx, row) in slot.rows.iter().enumerate() {
                if row.locked.is_none()
                    && row.disabled_reason.is_none()
                    && !row.choices.is_empty()
                {
                    answers[idx] = Some('^');
                }
            }
            Ok(answers)
        }
        MoveInPicks::Tokens(tokens) => {
            // Sentinel conflict detection.
            let has_all = tokens.iter().any(|t| t.selector == "__all__");
            let has_none = tokens.iter().any(|t| t.selector == "__none__");
            if has_all || has_none {
                return Err(CaptureError {
                    kind: super::output::CaptureErrorKind::Usage,
                    message: "conflicting --move-in picks; use none or all alone, or list rows".to_string(),
                    code: Some("move_choice_conflict".to_string()),
                    extra: Some(extra_with_report()),
                    human_unblocked: Vec::new(),
                    human_keep_moving_folds: Vec::new(),
                    human_relation_groups: Vec::new(),
                    human_relation_held: Vec::new(),
                    human_relation_check: None,
                    human_note_pull_rows: Vec::new(),
                    human_note_pull_groups: Vec::new(),
                    typed: None,
                });
            }
            // Duplicate row with both sigils -> conflict.
            {
                let mut by_row: HashMap<usize, char> = HashMap::new();
                for token in tokens {
                    if let Some(idx) = token_to_row(slot, token) {
                        if let Some(prev) = by_row.get(&idx) {
                            if *prev != token.sigil {
                                return Err(CaptureError {
                                    kind: super::output::CaptureErrorKind::Usage,
                                    message: format!(
                                        "conflicting --move-in picks for row {}",
                                        slot.rows[idx].n
                                    ),
                                    code: Some("move_choice_conflict".to_string()),
                                    extra: Some(extra_with_report()),
                                    human_unblocked: Vec::new(),
                                    human_keep_moving_folds: Vec::new(),
                                    human_relation_groups: Vec::new(),
                                    human_relation_held: Vec::new(),
                                    human_relation_check: None,
                                    human_note_pull_rows: Vec::new(),
                                    human_note_pull_groups: Vec::new(),
                                    typed: None,
                                });
                            }
                        } else {
                            by_row.insert(idx, token.sigil);
                        }
                    }
                }
            }
            // Resolve each token to a row.
            let mut answers: Vec<Option<char>> = vec![None; slot.rows.len()];
            let mut bad: Vec<String> = Vec::new();
            for token in tokens {
                match token_to_row(slot, token) {
                    Some(idx) => {
                        let row = &slot.rows[idx];
                        if row.locked.is_some() || row.disabled_reason.is_some()
                        {
                            return Err(CaptureError {
                                kind: super::output::CaptureErrorKind::Io,
                                message: format!(
                                    "refusing --move-in pick for row {}: {}",
                                    row.n,
                                    row.locked
                                        .as_ref()
                                        .map(|l| format!(
                                            "already moving on line {}",
                                            l.item_line
                                        ))
                                        .unwrap_or_else(|| row
                                            .disabled_reason
                                            .clone()
                                            .unwrap_or_default())
                                ),
                                code: Some("move_choice_refused".to_string()),
                                extra: Some(extra_with_report()),
                                human_unblocked: Vec::new(),
                                human_keep_moving_folds: Vec::new(),
                                human_relation_groups: Vec::new(),
                                human_relation_held: Vec::new(),
                                human_relation_check: None,
                                human_note_pull_rows: Vec::new(),
                                human_note_pull_groups: Vec::new(),
                                typed: None,
                            });
                        }
                        answers[idx] = Some(token.sigil);
                    }
                    None => {
                        bad.push(format!(
                            "{}{}",
                            if token.sigil == ':' { ":" } else { "" },
                            token.selector
                        ));
                    }
                }
            }
            if !bad.is_empty() {
                return Err(CaptureError {
                    kind: super::output::CaptureErrorKind::Usage,
                    message: format!(
                        "stale, unknown, or out-of-range --move-in pick: {}",
                        bad.join(", ")
                    ),
                    code: Some("move_choice_stale".to_string()),
                    extra: Some(extra_with_report()),
                    human_unblocked: Vec::new(),
                    human_keep_moving_folds: Vec::new(),
                    human_relation_groups: Vec::new(),
                    human_relation_held: Vec::new(),
                    human_relation_check: None,
                    human_note_pull_rows: Vec::new(),
                    human_note_pull_groups: Vec::new(),
                    typed: None,
                });
            }
            Ok(answers)
        }
    }
}

fn token_to_row(slot: &MoveChoiceSlot, token: &MoveInPick) -> Option<usize> {
    if token.is_number {
        if let Ok(n) = token.selector.parse::<usize>() {
            return slot.rows.iter().position(|row| row.n == n);
        }
        return None;
    }
    // Key match.
    slot.rows.iter().position(|row| row.key == token.selector)
}

/// Human `choose with` hint.
pub(crate) fn choose_hint() -> &'static str {
    "choose with -m 1,3 (move), -m :2 (move & link), or -m none"
}

/// Batch-level planning: build slots, enforce §3.7 order, and report.
///
/// Returns the `move_choice` report for successes. Refusals return a
/// `CaptureError` whose `extra` carries the same report so clients can
/// reopen the card.
///
/// Kept for direct report callers; the batch pre-pass is
/// `prepare_move_choice`, which also mints and splices.
#[allow(dead_code)]
pub(super) fn plan_batch_move_choice(
    request: &super::cli::CaptureRequest,
    parsed_items: &[crate::native::capture_language::ParsedCaptureItem],
    planner: &mut super::batch::CaptureBatchPlanner,
    today: NaiveDate,
    dependency_ctx: &mut super::dependencies::DependencyContext,
    warnings: &mut Vec<super::output::CaptureWarning>,
) -> Result<MoveChoiceReport, super::output::CaptureError> {
    use super::output::{CaptureError, CaptureErrorKind};
    let locks = collect_move_locks(&request.raw_text, parsed_items);

    // Build one slot per `- ^^` item.
    let mut slots: Vec<MoveChoiceSlot> = Vec::new();
    for parsed_item in parsed_items {
        let has_choice = parsed_item
            .parsed
            .sub_bullets
            .iter()
            .any(|b| b.move_choice.is_some());
        if !has_choice {
            continue;
        }
        // Only sealed project-note items reach here; others were already
        // refused lexically. Skip non-project items defensively.
        let is_project_note = matches!(
            parsed_item.parsed.kind,
            crate::native::capture_language::CaptureKind::ProjectNote { .. }
        );
        if !is_project_note {
            continue;
        }
        if let Some(slot) = build_slot(
            &request.bob_dir,
            &request.raw_text,
            &parsed_item.parsed,
            parsed_item.index,
            planner,
            today,
            &locks,
            dependency_ctx,
        )? {
            slots.push(slot);
        }
    }

    if slots.is_empty() {
        if request.move_in.is_some() {
            let report = MoveChoiceReport::empty();
            let mut extra = serde_json::Map::new();
            extra.insert(
                "move_choice".to_string(),
                serde_json::to_value(&report).expect("serialize move_choice"),
            );
            return Err(CaptureError {
                kind: CaptureErrorKind::Usage,
                message: "--move-in answers no - ^^ move choice in this draft"
                    .to_string(),
                code: Some("move_choice_unasked".to_string()),
                extra: Some(extra),
                human_unblocked: Vec::new(),
                human_keep_moving_folds: Vec::new(),
                human_relation_groups: Vec::new(),
                human_relation_held: Vec::new(),
                human_relation_check: None,
                human_note_pull_rows: Vec::new(),
                human_note_pull_groups: Vec::new(),
                typed: None,
            });
        }
        return Ok(MoveChoiceReport::empty());
    }

    // v1 allows exactly one slot; the lexer already refuses duplicates.
    // Handle each slot in order; the first refusal wins.
    let mut finished_slots: Vec<MoveChoiceSlot> = Vec::new();
    for mut slot in slots {
        let selectable = slot.rows.iter().any(|row| {
            row.locked.is_none()
                && row.disabled_reason.is_none()
                && !row.choices.is_empty()
        });
        let no_selectable = slot.rows.is_empty() || !selectable;
        if no_selectable {
            if request.dry_run {
                warnings.push(
                    format!(
                        "no selectable tasks in {} for - ^^ on line {}",
                        slot.parent, slot.item_line
                    )
                    .into(),
                );
                slot.answered = false;
                slot.moved_count = 0;
                slot.linked_count = 0;
                finished_slots.push(slot);
                continue;
            }
            let report = MoveChoiceReport::with_slots(vec![slot.clone()]);
            let mut extra = serde_json::Map::new();
            extra.insert(
                "move_choice".to_string(),
                serde_json::to_value(&report).expect("serialize move_choice"),
            );
            return Err(CaptureError {
                kind: CaptureErrorKind::Io,
                message: format!(
                    "no selectable tasks in {} for - ^^ on line {}",
                    slot.parent, slot.item_line
                ),
                code: Some("move_choice_empty".to_string()),
                extra: Some(extra),
                human_unblocked: Vec::new(),
                human_keep_moving_folds: Vec::new(),
                human_relation_groups: Vec::new(),
                human_relation_held: Vec::new(),
                human_relation_check: None,
                human_note_pull_rows: Vec::new(),
                human_note_pull_groups: Vec::new(),
                typed: None,
            });
        }

        // Resolve answers (required / conflict / stale / refused).
        let answers =
            resolve_picks(&slot, request.move_in.as_ref(), request.dry_run)?;
        // `unused_project_note_pomodoro` backstop: only when answered.
        if slot.name_needs_link && request.move_in.is_some() {
            let has_linking_pick =
                answers.iter().enumerate().any(|(idx, ans)| {
                    ans == &Some(':')
                        && slot.rows[idx]
                            .choices
                            .iter()
                            .any(|c| c.sigil == ":" && c.adds_link)
                });
            if !has_linking_pick {
                let pomodoro =
                    slot.pomodoro_name.as_deref().unwrap_or_default();
                let report = MoveChoiceReport::with_slots(vec![{
                    let mut s = slot.clone();
                    apply_answers(&mut s, &answers);
                    s
                }]);
                let mut extra = serde_json::Map::new();
                extra.insert(
                    "move_choice".to_string(),
                    serde_json::to_value(&report)
                        .expect("serialize move_choice"),
                );
                return Err(CaptureError {
                    kind: CaptureErrorKind::Usage,
                    message: crate::native::capture_language::unused_project_note_pomodoro_error(pomodoro),
                    code: Some("unused_project_note_pomodoro".to_string()),
                    extra: Some(extra),
                    human_unblocked: Vec::new(),
                    human_keep_moving_folds: Vec::new(),
                    human_relation_groups: Vec::new(),
                    human_relation_held: Vec::new(),
                    human_relation_check: None,
                    human_note_pull_rows: Vec::new(),
                    human_note_pull_groups: Vec::new(),
                    typed: None,
                });
            }
        }

        // Answered runs: mark answered (explicit `none` has no picks but
        // is still answered) and report with moved/linked counts.
        // Execution (mint, splice, offers, `via`) happens in `cli-execute`
        // via `prepare_move_choice` before item planning; this report path
        // is for callers that plan without that pre-pass.
        apply_answers(&mut slot, &answers);
        if request.move_in.is_some() {
            slot.answered = true;
        }
        finished_slots.push(slot);
    }

    Ok(MoveChoiceReport::with_slots(finished_slots))
}

fn apply_answers(slot: &mut MoveChoiceSlot, answers: &[Option<char>]) {
    let mut moved = 0;
    let mut linked = 0;
    for (row, ans) in slot.rows.iter_mut().zip(answers.iter()) {
        row.answer = ans.map(|c| c.to_string());
        if let Some(sigil) = ans {
            moved += 1;
            if *sigil == ':' {
                // Count only linking picks.
                let adds =
                    row.choices.iter().any(|c| c.sigil == ":" && c.adds_link);
                if adds {
                    linked += 1;
                }
            }
        }
    }
    slot.answered = answers.iter().any(|a| a.is_some());
    // Unanswered dry run stays answered=false with zero counts.
    // Explicit `-m none` also resolves to all None; the caller marks
    // answered=true when the flag was present.
    slot.moved_count = moved;
    slot.linked_count = linked;
}

/// `cli-execute` pre-pass: build slots from the pre-item staged parent,
/// enforce the §3.7 refusal order, mint IDs for picked ID-less rows into the
/// staged parent, and splice picks into the item's sub-bullets at the `^^`
/// slot in parent document order.
///
/// Returns the final `move_choice` report (answered slots carry answers and
/// moved/linked counts). Mutates `parsed_items` (synthetic `via: Choice`
/// moves replace the `^^` marker) and stages parent mint edits into
/// `planner`. Refusals return before any item planning, write-free.
#[allow(clippy::too_many_arguments)]
pub(super) fn prepare_move_choice(
    request: &super::cli::CaptureRequest,
    parsed_items: &mut Vec<crate::native::capture_language::ParsedCaptureItem>,
    planner: &mut super::batch::CaptureBatchPlanner,
    today: NaiveDate,
    dependency_ctx: &mut super::dependencies::DependencyContext,
    warnings: &mut Vec<super::output::CaptureWarning>,
) -> Result<MoveChoiceReport, super::output::CaptureError> {
    use super::output::{CaptureError, CaptureErrorKind};
    let locks = collect_move_locks(&request.raw_text, parsed_items);

    // Build one slot per `- ^^` item from the pre-item staged parent.
    let mut built: Vec<(usize, MoveChoiceSlot)> = Vec::new();
    for parsed_item in parsed_items.iter() {
        let has_choice = parsed_item
            .parsed
            .sub_bullets
            .iter()
            .any(|b| b.move_choice.is_some());
        if !has_choice {
            continue;
        }
        let is_project_note = matches!(
            parsed_item.parsed.kind,
            crate::native::capture_language::CaptureKind::ProjectNote { .. }
        );
        if !is_project_note {
            continue;
        }
        if let Some(slot) = build_slot(
            &request.bob_dir,
            &request.raw_text,
            &parsed_item.parsed,
            parsed_item.index,
            planner,
            today,
            &locks,
            dependency_ctx,
        )? {
            built.push((parsed_item.index, slot));
        }
    }

    if built.is_empty() {
        if request.move_in.is_some() {
            let report = MoveChoiceReport::empty();
            let mut extra = serde_json::Map::new();
            extra.insert(
                "move_choice".to_string(),
                serde_json::to_value(&report).expect("serialize move_choice"),
            );
            return Err(CaptureError {
                kind: CaptureErrorKind::Usage,
                message: "--move-in answers no - ^^ move choice in this draft"
                    .to_string(),
                code: Some("move_choice_unasked".to_string()),
                extra: Some(extra),
                human_unblocked: Vec::new(),
                human_keep_moving_folds: Vec::new(),
                human_relation_groups: Vec::new(),
                human_relation_held: Vec::new(),
                human_relation_check: None,
                human_note_pull_rows: Vec::new(),
                human_note_pull_groups: Vec::new(),
                typed: None,
            });
        }
        return Ok(MoveChoiceReport::empty());
    }

    let mut finished_slots: Vec<MoveChoiceSlot> = Vec::new();
    // Process in item order; the first refusal wins. Mint and splice each
    // answered slot before the next so shared parents see earlier mints.
    built.sort_by_key(|(index, _)| *index);
    for (_, mut slot) in built {
        let selectable = slot.rows.iter().any(|row| {
            row.locked.is_none()
                && row.disabled_reason.is_none()
                && !row.choices.is_empty()
        });
        let no_selectable = slot.rows.is_empty() || !selectable;
        if no_selectable {
            if request.dry_run {
                warnings.push(
                    format!(
                        "no selectable tasks in {} for - ^^ on line {}",
                        slot.parent, slot.item_line
                    )
                    .into(),
                );
                slot.answered = false;
                slot.moved_count = 0;
                slot.linked_count = 0;
                finished_slots.push(slot);
                continue;
            }
            let report = MoveChoiceReport::with_slots(vec![slot.clone()]);
            let mut extra = serde_json::Map::new();
            extra.insert(
                "move_choice".to_string(),
                serde_json::to_value(&report).expect("serialize move_choice"),
            );
            return Err(CaptureError {
                kind: CaptureErrorKind::Io,
                message: format!(
                    "no selectable tasks in {} for - ^^ on line {}",
                    slot.parent, slot.item_line
                ),
                code: Some("move_choice_empty".to_string()),
                extra: Some(extra),
                human_unblocked: Vec::new(),
                human_keep_moving_folds: Vec::new(),
                human_relation_groups: Vec::new(),
                human_relation_held: Vec::new(),
                human_relation_check: None,
                human_note_pull_rows: Vec::new(),
                human_note_pull_groups: Vec::new(),
                typed: None,
            });
        }

        let answers =
            resolve_picks(&slot, request.move_in.as_ref(), request.dry_run)?;
        if slot.name_needs_link && request.move_in.is_some() {
            let has_linking_pick =
                answers.iter().enumerate().any(|(idx, ans)| {
                    ans == &Some(':')
                        && slot.rows[idx]
                            .choices
                            .iter()
                            .any(|c| c.sigil == ":" && c.adds_link)
                });
            if !has_linking_pick {
                let pomodoro =
                    slot.pomodoro_name.as_deref().unwrap_or_default();
                let report = MoveChoiceReport::with_slots(vec![{
                    let mut s = slot.clone();
                    apply_answers(&mut s, &answers);
                    s
                }]);
                let mut extra = serde_json::Map::new();
                extra.insert(
                    "move_choice".to_string(),
                    serde_json::to_value(&report)
                        .expect("serialize move_choice"),
                );
                return Err(CaptureError {
                    kind: CaptureErrorKind::Usage,
                    message: crate::native::capture_language::unused_project_note_pomodoro_error(pomodoro),
                    code: Some("unused_project_note_pomodoro".to_string()),
                    extra: Some(extra),
                    human_unblocked: Vec::new(),
                    human_keep_moving_folds: Vec::new(),
                    human_relation_groups: Vec::new(),
                    human_relation_held: Vec::new(),
                    human_relation_check: None,
                    human_note_pull_rows: Vec::new(),
                    human_note_pull_groups: Vec::new(),
                    typed: None,
                });
            }
        }

        if request.move_in.is_none() {
            // Unanswered dry run: planned as none, no mint or splice.
            apply_answers(&mut slot, &answers);
            finished_slots.push(slot);
            continue;
        }

        // Answered: mint ID-less picks into the staged parent, then splice
        // synthetic `via: Choice` moves at the `^^` slot in parent order.
        mint_and_splice(request, &slot, &answers, parsed_items, planner)?;
        apply_answers(&mut slot, &answers);
        slot.answered = true;
        finished_slots.push(slot);
    }

    Ok(MoveChoiceReport::with_slots(finished_slots))
}

/// Mint IDs for picked ID-less rows into the staged parent and splice
/// synthetic `via: Choice` moves at the `^^` position in parent order.
fn mint_and_splice(
    request: &super::cli::CaptureRequest,
    slot: &MoveChoiceSlot,
    answers: &[Option<char>],
    parsed_items: &mut Vec<crate::native::capture_language::ParsedCaptureItem>,
    planner: &mut super::batch::CaptureBatchPlanner,
) -> Result<(), super::output::CaptureError> {
    use super::output::CaptureError;
    // Collect picks in parent document order (slot.rows order).
    let mut picks: Vec<(usize, char)> = Vec::new();
    for (idx, ans) in answers.iter().enumerate() {
        if let Some(sigil) = ans {
            picks.push((idx, *sigil));
        }
    }
    if picks.is_empty() {
        return Ok(());
    }

    let parsed_index = slot.item - 1;
    let parsed_item = parsed_items
        .iter()
        .find(|p| p.index == parsed_index)
        .ok_or_else(|| {
            CaptureError::io(
                "move choice invariant failed: choice item is missing",
            )
        })?;
    // Used IDs: parent IDs + new note authored IDs + prj + other mints.
    let source_path = request.bob_dir.join(&slot.parent);
    let parent_contents = planner.current_contents(&source_path)?.ok_or_else(|| {
        CaptureError::io(format!(
            "cannot create a project note under {}: note does not exist (run 'bob capture-targets' to list routable notes)",
            slot.parent
        ))
    })?;
    let settings = note_tasks::read_settings(&request.bob_dir);
    let scan = note_tasks::scan(&parent_contents, &settings);
    let mut used: HashSet<String> = HashSet::new();
    for task in scan.tasks() {
        if let Some(id) = task.block_id.as_deref() {
            used.insert(id.to_string());
        }
    }
    for sub in &parsed_item.parsed.sub_bullets {
        if let Some(task) = sub.task_id.as_ref() {
            used.insert(task.block_id.clone());
        }
        if let Some(movement) = sub.move_task.as_ref()
            && let Some(id) = movement.block_id.as_deref()
        {
            used.insert(id.to_string());
        }
    }
    used.insert("prj".to_string());

    // Resolve final IDs in parent order, reusing the report mint when free.
    let mut finals: HashMap<usize, (String, bool)> = HashMap::new();
    for (row_idx, _) in &picks {
        let row = &slot.rows[*row_idx];
        if let Some(id) = row.block_id.as_deref() {
            finals.insert(*row_idx, (id.to_string(), false));
        } else {
            let suggested = row.mint.clone();
            let chosen = if let Some(candidate) = suggested
                && !used.contains(&candidate)
            {
                candidate
            } else {
                let used_ref: HashSet<&str> =
                    used.iter().map(String::as_str).collect();
                capture_block_ids::mint_block_id(&row.text, &used_ref)
            };
            used.insert(chosen.clone());
            finals.insert(*row_idx, (chosen, true));
        }
    }

    // Stage parent mint edits for ID-less picks.
    let mut lines: Vec<String> =
        parent_contents.lines().map(str::to_string).collect();
    let had_trailing_newline = parent_contents.ends_with('\n');
    for (row_idx, _) in &picks {
        let (final_id, is_minted) = finals.get(row_idx).expect("final");
        if !is_minted {
            continue;
        }
        let row = &slot.rows[*row_idx];
        let line_idx = row.line.saturating_sub(1);
        let Some(line) = lines.get_mut(line_idx) else {
            return Err(CaptureError::io(
                "move choice invariant failed: parent line is missing",
            ));
        };
        if line.contains(&format!("^{final_id}")) {
            continue;
        }
        let trimmed = line.trim_end().to_string();
        *line = format!("{trimmed} ^{final_id}");
    }
    let mut staged = lines.join("\n");
    if had_trailing_newline {
        staged.push('\n');
    }
    planner.stage(&source_path, staged)?;

    // Splice synthetic moves at the `^^` index in parent order.
    let target = parsed_items
        .iter_mut()
        .find(|p| p.index == parsed_index)
        .expect("choice item");
    let choice_pos = target
        .parsed
        .sub_bullets
        .iter()
        .position(|b| b.move_choice.is_some())
        .ok_or_else(|| {
            CaptureError::io(
                "move choice invariant failed: ^^ marker is missing",
            )
        })?;
    let choice_range = target.parsed.sub_bullets[choice_pos]
        .move_choice
        .as_ref()
        .expect("choice range")
        .range;
    let mut synthetics: Vec<
        crate::native::capture_language::AuthoredSubBullet,
    > = Vec::new();
    for (row_idx, sigil) in picks {
        let (final_id, is_minted) = finals.remove(&row_idx).expect("final");
        synthetics.push(crate::native::capture_language::AuthoredSubBullet {
            body: String::new(),
            depth: crate::native::capture_language::AuthoredDepth::First,
            task_id: None,
            move_task: Some(crate::native::capture_language::MoveBullet {
                sigil,
                block_id: Some(final_id),
                range: choice_range,
                via: crate::native::capture_language::MoveVia::Choice,
                minted: is_minted,
            }),
            move_choice: None,
            drawer: None,
        });
    }
    target
        .parsed
        .sub_bullets
        .splice(choice_pos..=choice_pos, synthetics);
    Ok(())
}

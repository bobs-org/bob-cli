//! Capture-side successor-offer verdicts: `-x/--complete-offer` and
//! `-X/--cancel-offer` parsing, probe-based key resolution, conflict,
//! stale, and refused errors, the verdict post-pass, and `offer_closes`
//! receipts (`docs/task-dependencies.md` §12.5, §12.14, §12.22).

use super::*;
use crate::native::task_complete::{
    self as engine, offer_close, SuccessorSelection,
};
use std::collections::{BTreeMap, BTreeSet};

/// Staged card verdict for one offer row, resolved to its stable key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OfferVerdictRequest {
    pub(super) key: String,
    pub(super) outcome: OfferVerdictOutcome,
    pub(super) reason: String,
}

/// Done (`-x`) or Cancel (`-X`) for one resolved verdict row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OfferVerdictOutcome {
    Done,
    Cancelled,
}

impl OfferVerdictOutcome {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Split every `-x/--complete-offer` value on commas. Each token names
/// one offer by its number or stable key.
pub(super) fn parse_complete_offer_args(
    raws: &[String],
) -> Result<Vec<String>, CaptureError> {
    let mut selectors = Vec::new();
    for raw in raws {
        for token in raw.split(',') {
            let token = token.trim();
            if token.is_empty() {
                return Err(CaptureError::usage(
                    "--complete-offer has an empty token; use a comma list of offer numbers and keys",
                ));
            }
            selectors.push(token.to_string());
        }
    }
    Ok(selectors)
}

/// Split every `-X/--cancel-offer` value on the first `=`, like `-i`.
/// A missing `=` is a blank reason; commas in the reason stay literal.
pub(super) fn parse_cancel_offer_args(
    raws: &[String],
) -> Result<Vec<(String, String)>, CaptureError> {
    let mut selectors = Vec::new();
    for raw in raws {
        let (selector, reason) = match raw.split_once('=') {
            Some((selector, reason)) => (selector, reason),
            None => (raw.as_str(), ""),
        };
        let selector = selector.trim();
        if selector.is_empty() {
            return Err(CaptureError::usage(
                "--cancel-offer must be <N|KEY>[=REASON]",
            ));
        }
        selectors.push((selector.to_string(), collapse_reason(reason)));
    }
    Ok(selectors)
}

fn collapse_reason(reason: &str) -> String {
    reason.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Map raw verdict selectors to stable offer keys through the probe
/// numbering. Numbers index the offers the same command reports without
/// a selection; anything else is already a key.
fn resolve_verdict_keys(
    selectors: &[String],
    n_map: &BTreeMap<String, usize>,
    surviving: &BTreeSet<String>,
) -> (Vec<String>, Vec<String>) {
    let number_to_key: BTreeMap<usize, &String> =
        n_map.iter().map(|(key, n)| (*n, key)).collect();
    let mut keys = Vec::new();
    let mut bad = Vec::new();
    for selector in selectors {
        if selector.bytes().all(|byte| byte.is_ascii_digit()) {
            match selector
                .parse::<usize>()
                .ok()
                .and_then(|number| number_to_key.get(&number))
            {
                Some(key) => keys.push((*key).clone()),
                None => bad.push(selector.clone()),
            }
        } else if surviving.contains(selector) {
            keys.push(selector.clone());
        } else {
            bad.push(selector.clone());
        }
    }
    (keys, bad)
}

/// Resolve `-x`/`-X` selectors against the probe offer list, detect
/// conflicts with each other and with `--pull-in`, and refuse
/// ineligible rows. Returns stable-keyed verdicts in flag order with
/// duplicate Done selectors folded into one.
pub(super) fn resolve_offer_verdicts(
    done_selectors: &[String],
    cancel_selectors: &[(String, String)],
    pull_in_arg: Option<&PullInPicks>,
    pull_in_keys: &BTreeSet<String>,
    items: &[PlannedCaptureItem],
) -> Result<Vec<OfferVerdictRequest>, CaptureError> {
    let n_map = super::pull_in::offer_n_map(items);
    let surviving = super::pull_in::surviving_keys(items);
    let pull_in = super::pull_in::assemble_pull_in(items, None, &[]);
    let offers = flat_offers(items);
    let (done_keys, mut bad) =
        resolve_verdict_keys(done_selectors, &n_map, &surviving);
    let (cancel_keys, mut cancel_bad) = resolve_verdict_keys(
        &cancel_selectors
            .iter()
            .map(|(selector, _)| selector.clone())
            .collect::<Vec<_>>(),
        &n_map,
        &surviving,
    );
    bad.append(&mut cancel_bad);
    if !bad.is_empty() {
        return Err(CaptureError::offer_close_stale(&pull_in, &offers, &bad));
    }
    // One verdict per row: a second `-X` for a row conflicts even with
    // the same reason, and a Done plus a Cancel conflict. A repeated
    // `-x` for the same row folds into one Done.
    let mut seen_cancel: BTreeSet<String> = BTreeSet::new();
    for key in &cancel_keys {
        if seen_cancel.contains(key) {
            return Err(CaptureError::offer_verdict_conflict(
                &pull_in,
                &offers,
                format!(
                    "conflicting --cancel-offer answers for offer {key}; pass one verdict per offer"
                ),
            ));
        }
        seen_cancel.insert(key.clone());
    }
    for key in &done_keys {
        if seen_cancel.contains(key) {
            return Err(CaptureError::offer_verdict_conflict(
                &pull_in,
                &offers,
                format!(
                    "conflicting verdict answers for offer {key}; pass it to only one of --pull-in, --complete-offer, or --cancel-offer"
                ),
            ));
        }
    }
    if let Some(explicit) = pull_in_keys_for_conflict(pull_in_arg, pull_in_keys)
    {
        for key in done_keys.iter().chain(cancel_keys.iter()) {
            if explicit.contains(key) {
                return Err(CaptureError::offer_verdict_conflict(
                    &pull_in,
                    &offers,
                    format!(
                        "conflicting verdict answers for offer {key}; pass it to only one of --pull-in, --complete-offer, or --cancel-offer"
                    ),
                ));
            }
        }
    }
    let mut verdicts = Vec::new();
    let mut done_emitted: BTreeSet<String> = BTreeSet::new();
    for key in done_keys {
        if done_emitted.insert(key.clone()) {
            verdicts.push(OfferVerdictRequest {
                key,
                outcome: OfferVerdictOutcome::Done,
                reason: String::new(),
            });
        }
    }
    for (index, key) in cancel_keys.into_iter().enumerate() {
        verdicts.push(OfferVerdictRequest {
            key,
            outcome: OfferVerdictOutcome::Cancelled,
            reason: cancel_selectors[index].1.clone(),
        });
    }
    // Ineligible rows refuse with the engine's `why`; the batch writes
    // nothing.
    refuse_ineligible_verdicts(&verdicts, items)?;
    Ok(verdicts)
}

/// Human refusal line for an ineligible verdict row: the engine's `why`
/// code rendered in the card's copy.
fn refused_message(
    key: &str,
    n: usize,
    outcome: OfferVerdictOutcome,
    why: &str,
    text: &str,
    status_name: &str,
) -> String {
    let head =
        format!("offer {n} ({key}) cannot be marked {}", outcome.as_str());
    match why {
        "new_task" => format!(
            "{head}: “{text}” is new in this capture — drop it from the draft instead"
        ),
        "recurring" => format!(
            "{head}: “{text}” repeats — close it in Obsidian so Tasks writes the next one"
        ),
        "status" => format!(
            "{head}: “{text}” is {status_name} — only open tasks close here"
        ),
        "muted" => format!(
            "{head}: “{text}” is not selectable in this card — only open offers close here"
        ),
        _ => format!("{head}: the row changed since the preview; rerun to see current offers"),
    }
}

/// Explicit `--pull-in` keys for the verdict conflict check. `all` and
/// `none` never conflict: `all` skips verdict rows and `none` selects
/// nothing.
fn pull_in_keys_for_conflict<'a>(
    pull_in: Option<&PullInPicks>,
    resolved: &'a BTreeSet<String>,
) -> Option<&'a BTreeSet<String>> {
    match pull_in {
        Some(PullInPicks::Tokens { .. }) => Some(resolved),
        _ => None,
    }
}

/// Stable-keyed verdict rows that an inbox-route answer must not name:
/// `(note_path, block_id, 1-based line)`.
pub(super) fn verdict_row_identities(
    items: &[PlannedCaptureItem],
) -> Vec<(String, String, usize)> {
    let mut identities = Vec::new();
    for item in items {
        for row in super::output::all_offer_rows(&item.result) {
            let Some(offer) = row.offer.as_ref() else {
                continue;
            };
            if offer.verdict.is_none() {
                continue;
            }
            identities.push((
                row.note_path.clone(),
                row.block_id.clone(),
                row.line,
            ));
        }
    }
    identities
}

fn find_offer_row<'a>(
    items: &'a [PlannedCaptureItem],
    key: &str,
) -> Option<&'a super::output::TaskCompleteUnblockedJson> {
    items
        .iter()
        .flat_map(|item| super::output::all_offer_rows(&item.result))
        .find(|row| row.offer.as_ref().is_some_and(|offer| offer.key == key))
}

/// Flat offer rows for refusal payloads, in row order.
fn flat_offers(
    items: &[PlannedCaptureItem],
) -> Vec<super::output::TaskCompleteUnblockedJson> {
    items
        .iter()
        .flat_map(|item| super::output::all_offer_rows(&item.result))
        .cloned()
        .collect()
}

/// Re-validate resolved verdicts against the final batch: a vanished row
/// is stale and a newly refusing row is refused. The batch writes
/// nothing. The probe already validated the same verdicts, so this only
/// fires when planning itself moved the rows.
pub(super) fn revalidate_offer_verdicts(
    verdicts: &[OfferVerdictRequest],
    items: &[PlannedCaptureItem],
) -> Result<(), CaptureError> {
    if verdicts.is_empty() {
        return Ok(());
    }
    refuse_ineligible_verdicts(verdicts, items)
}

/// Shared stale/refused check used by resolution (probe batch) and
/// revalidation (final batch).
fn refuse_ineligible_verdicts(
    verdicts: &[OfferVerdictRequest],
    items: &[PlannedCaptureItem],
) -> Result<(), CaptureError> {
    let pull_in = super::pull_in::assemble_pull_in(items, None, &[]);
    let offers = flat_offers(items);
    for verdict in verdicts {
        let Some(row) = find_offer_row(items, &verdict.key) else {
            return Err(CaptureError::offer_close_stale(
                &pull_in,
                &offers,
                &[verdict.key.clone()],
            ));
        };
        let why = row.offer.as_ref().and_then(|offer| {
            offer.close.as_ref().map(|close| match verdict.outcome {
                OfferVerdictOutcome::Done => {
                    (close.complete, close.why.clone())
                }
                OfferVerdictOutcome::Cancelled => {
                    (close.cancel, close.why.clone())
                }
            })
        });
        match why {
            Some((true, _)) => {}
            Some((false, refusal)) => {
                let code =
                    refusal.unwrap_or_else(|| "changed_identity".to_string());
                let n = row.offer.as_ref().map(|offer| offer.n).unwrap_or(0);
                let message = refused_message(
                    &verdict.key,
                    n,
                    verdict.outcome,
                    &code,
                    &row.text,
                    &row.previous_status_name,
                );
                return Err(CaptureError::offer_close_refused(
                    &pull_in,
                    &offers,
                    &verdict.key,
                    n,
                    verdict.outcome.as_str(),
                    &code,
                    message,
                ));
            }
            None => {
                return Err(CaptureError::offer_close_stale(
                    &pull_in,
                    &offers,
                    &[verdict.key.clone()],
                ));
            }
        }
    }
    Ok(())
}

/// Refuse an inbox-route answer that names a card-resolved task: the row
/// is Done or Cancelled by this same gesture, so routing it is a
/// conflict (`offer_verdict_conflict`, usage, exit 2, writes nothing).
pub(super) fn check_inbox_route_verdict_conflict(
    report: &super::inbox_route::InboxRouteReport,
    picks: &[super::cli::InboxRoutePick],
    verdict_identities: &[(String, String, usize)],
) -> Result<(), CaptureError> {
    if verdict_identities.is_empty() {
        return Ok(());
    }
    for pick in picks {
        let super::cli::InboxRoutePick::Answer { selector, .. } = pick else {
            continue;
        };
        // Numeric selectors are positions in the post-filter route list.
        // Only explicit keys can conflict with a card-resolved task, and
        // this check deliberately runs against the unfiltered report.
        if selector.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let Some(candidate) = report
            .candidates
            .iter()
            .find(|candidate| candidate.key == *selector)
        else {
            continue;
        };
        if inbox_candidate_matches_verdict(candidate, verdict_identities) {
            return Err(CaptureError::offer_verdict_conflict(
                &super::output::PullInJson::empty(),
                &[],
                format!(
                    "conflicting answers for {}: it is resolved Done or Cancelled by --complete-offer/--cancel-offer in this capture, so --inbox-route cannot route it",
                    selector,
                ),
            ));
        }
    }
    Ok(())
}

/// Remove card-resolved rows from route choices after keyed conflicts have
/// been checked against the original report. Numeric route selectors are
/// positions in this filtered list, so every survivor receives its new
/// one-based number here.
pub(super) fn drop_verdict_rows_from_inbox_route(
    report: &mut super::inbox_route::InboxRouteReport,
    verdict_identities: &[(String, String, usize)],
) {
    report.candidates.retain(|candidate| {
        !inbox_candidate_matches_verdict(candidate, verdict_identities)
    });
    report.candidate_count = report.candidates.len();
    for (index, candidate) in report.candidates.iter_mut().enumerate() {
        candidate.n = index + 1;
    }
    if report.candidates.is_empty() {
        report.destinations = None;
    }
}

fn inbox_candidate_matches_verdict(
    candidate: &super::inbox_route::InboxRouteCandidate,
    verdict_identities: &[(String, String, usize)],
) -> bool {
    verdict_identities.iter().any(|(note, block_id, line)| {
        candidate.note_path == *note
            && ((!block_id.is_empty() && candidate.block_id == *block_id)
                || candidate.line == *line)
    })
}

/// Apply staged verdicts in the capture write pass: resolve every row
/// against the final staged snapshot, apply the root-only line edits,
/// run one recover-only cascade over the union, stage everything into
/// the batch planner, and return the `offer_closes` receipts in flag
/// order. Runs on the staged snapshot right after the successor net
/// post-pass and before inbox-route detection; the single
/// `write_staged_files` commit still guards external edits.
#[allow(clippy::too_many_arguments)]
pub(super) fn plan_offer_verdict_post_pass(
    verdicts: &[OfferVerdictRequest],
    items: &mut [PlannedCaptureItem],
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    dependency_ctx: &DependencyContext,
    today: chrono::NaiveDate,
    day_file: &Path,
    day_relative: &Path,
    dry_run: bool,
) -> Result<Vec<super::output::OfferCloseJson>, CaptureError> {
    if verdicts.is_empty() {
        return Ok(Vec::new());
    }
    refuse_ineligible_verdicts(verdicts, items)?;
    let tasks_settings =
        crate::native::task_status_hooks::read_tasks_settings(bob_dir);
    let note_settings = dependency_ctx.settings();
    // Cancel-log insertions shift later lines in the same note, so apply
    // lower rows first: descending line order per note.
    let mut order: Vec<usize> = (0..verdicts.len()).collect();
    order.sort_by(|left, right| {
        let left_row = find_offer_row(items, &verdicts[*left].key);
        let right_row = find_offer_row(items, &verdicts[*right].key);
        match (left_row, right_row) {
            (Some(left), Some(right)) => (right.note_path.clone(), right.line)
                .cmp(&(left.note_path.clone(), left.line)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
    });
    struct Applied {
        index: usize,
        note_path: PathBuf,
        previous_status: char,
        block_id: String,
        task_id: String,
        description: String,
        reason: Option<String>,
        cancel_log: Option<&'static str>,
        row_n: usize,
    }
    let mut applied: Vec<Applied> = Vec::new();
    for position in order {
        let verdict = &verdicts[position];
        let row = find_offer_row(items, &verdict.key).ok_or_else(|| {
            let pull_in = super::pull_in::assemble_pull_in(items, None, &[]);
            CaptureError::offer_close_stale(
                &pull_in,
                &flat_offers(items),
                &[verdict.key.clone()],
            )
        })?;
        let relative = PathBuf::from(&row.note_path);
        let absolute = bob_dir.join(&relative);
        let Some(source_text) = planner.peek_text(&absolute) else {
            let pull_in = super::pull_in::assemble_pull_in(items, None, &[]);
            return Err(CaptureError::offer_close_stale(
                &pull_in,
                &flat_offers(items),
                &[verdict.key.clone()],
            ));
        };
        let line_index = row.line.saturating_sub(1);
        let group = row
            .offer
            .as_ref()
            .map(|offer| offer.group)
            .unwrap_or("hand_off");
        let expected_line =
            engine::successors::raw_task_line(&source_text, line_index);
        let staged = super::task_complete::staged_overlay_map(bob_dir, planner);
        let dependencies = dependency_ctx.dependents_snapshot();
        let snapshot_complete = dependencies.snapshot_complete(&staged);
        let overlaid = dependencies.overlaid(staged);
        let snapshot = snapshot_with_source(&overlaid, &relative, &source_text);
        let index = super::task_complete::staged_note_index(
            bob_dir,
            planner,
            dependency_ctx,
        );
        let action = match verdict.outcome {
            OfferVerdictOutcome::Done => offer_close::OfferCloseAction::Done,
            OfferVerdictOutcome::Cancelled => {
                offer_close::OfferCloseAction::Cancel {
                    reason: verdict.reason.clone(),
                }
            }
        };
        let input = offer_close::OfferCloseInput {
            key: &verdict.key,
            path: &relative,
            line_index,
            expected_line: &expected_line,
            source_text: &source_text,
            group,
            is_new_task: false,
            muted: false,
            today,
            tasks_settings: &tasks_settings,
            note_settings,
            snapshot: &snapshot,
            snapshot_complete,
            day_relative,
            index: &index,
        };
        let plan = offer_close::plan_offer_close(&input, action).map_err(
            |refusal| {
                let pull_in =
                    super::pull_in::assemble_pull_in(items, None, &[]);
                let offers = flat_offers(items);
                match refusal {
                    offer_close::OfferCloseRefusal::ChangedIdentity => {
                        CaptureError::offer_close_stale(
                            &pull_in,
                            &offers,
                            &[verdict.key.clone()],
                        )
                    }
                    other => {
                        let code = other.as_str().to_string();
                        let n = row
                            .offer
                            .as_ref()
                            .map(|offer| offer.n)
                            .unwrap_or(0);
                        let message = refused_message(
                            &verdict.key,
                            n,
                            verdict.outcome,
                            &code,
                            &row.text,
                            &row.previous_status_name,
                        );
                        CaptureError::offer_close_refused(
                            &pull_in,
                            &offers,
                            &verdict.key,
                            n,
                            verdict.outcome.as_str(),
                            &code,
                            message,
                        )
                    }
                }
            },
        )?;
        let had_log = note_has_cancel_log(&source_text, line_index);
        let cancel_log = match verdict.outcome {
            OfferVerdictOutcome::Done => None,
            OfferVerdictOutcome::Cancelled => Some(cancel_log_outcome(
                plan.reason.as_deref().unwrap_or(""),
                had_log,
            )),
        };
        planner.stage(&absolute, plan.source_postimage)?;
        let parsed = crate::native::task_status_hooks::parse_tasks(
            &source_text,
            &tasks_settings,
        );
        let task = parsed.iter().find(|task| task.line_index == line_index);
        applied.push(Applied {
            index: position,
            note_path: relative,
            previous_status: plan.previous_status,
            block_id: plan.block_id.clone(),
            task_id: task
                .and_then(|task| task.task_id.clone())
                .unwrap_or_default(),
            description: task
                .map(|task| task.description.clone())
                .unwrap_or_default(),
            reason: plan.reason.clone(),
            cancel_log,
            row_n: row.offer.as_ref().map(|offer| offer.n).unwrap_or(0),
        });
    }
    // One recover-only cascade over the union of closed rows: dependents
    // recover with `link_unblocked: false` and are never offered.
    let staged = super::task_complete::staged_overlay_map(bob_dir, planner);
    let dependencies = dependency_ctx.dependents_snapshot();
    let snapshot_complete = dependencies.snapshot_complete(&staged);
    let snapshot = dependencies.overlaid(staged);
    let index = super::task_complete::staged_note_index(
        bob_dir,
        planner,
        dependency_ctx,
    );
    let completed: Vec<engine::successors::CompletedTask> = applied
        .iter()
        .map(|close| engine::successors::CompletedTask {
            relative_path: close.note_path.clone(),
            block_id: close.block_id.clone(),
            task_id: close.task_id.clone(),
            text: close.description.clone(),
            is_root: true,
        })
        .collect();
    let mut cancelled_projects = BTreeSet::new();
    for close in &applied {
        let verdict = &verdicts[close.index];
        if matches!(verdict.outcome, OfferVerdictOutcome::Cancelled)
            && crate::native::project_dependencies::is_project_task_id(
                (!close.block_id.is_empty()).then_some(close.block_id.as_str()),
            )
        {
            cancelled_projects.insert(close.note_path.clone());
        }
    }
    let recovery = engine::successors::plan_recovery_only(
        engine::successors::SuccessorInput {
            bob_dir: None,
            completed,
            snapshot,
            note_texts: Vec::new(),
            pre_day: None,
            post_day: None,
            closing: None,
            day_relative: day_relative.to_path_buf(),
            today,
            tasks_settings: &tasks_settings,
            note_settings,
            link_unblocked: false,
            keep_moving: false,
            snapshot_complete,
            selection: SuccessorSelection::None,
            index: &index,
            relation: None,
        },
        &cancelled_projects,
    );
    for (relative, text) in &recovery.changed_files {
        planner.stage(&bob_dir.join(relative), text.clone())?;
    }
    if let Some(day) = recovery.new_day_text.as_deref() {
        planner.stage(day_file, day.to_string())?;
    }
    // Receipts in flag order. Each recovered row is attributed to the
    // first verdict that names it, so a joint unblock is reported once.
    let mut attributed: Vec<Vec<&engine::successors::UnblockedRow>> =
        vec![Vec::new(); verdicts.len()];
    for row in &recovery.unblocked {
        for position in 0..verdicts.len() {
            let close = applied.iter().find(|close| close.index == position);
            let Some(close) = close else {
                continue;
            };
            let matches = row.unblocked_by.iter().any(|cause| {
                cause.note_path == close.note_path
                    && (if close.block_id.is_empty() {
                        cause.block_id.is_empty()
                            && cause.text == close.description
                    } else {
                        cause.block_id == close.block_id
                    })
            });
            if matches {
                attributed[position].push(row);
                break;
            }
        }
    }
    let mut closes: Vec<(usize, super::output::OfferCloseJson)> = Vec::new();
    for close in &applied {
        let verdict = &verdicts[close.index];
        let recovered: Vec<super::output::OfferCloseRecoveredJson> = attributed
            [close.index]
            .iter()
            .map(|row| {
                let clean = crate::native::note_tasks::clean_description(
                    &row.text,
                    &note_settings.global_filter,
                    (!row.block_id.is_empty()).then_some(row.block_id.as_str()),
                );
                super::output::OfferCloseRecoveredJson {
                    note_path: super::dependencies::display_relative(
                        &row.note_path,
                    ),
                    block_id: row.block_id.clone(),
                    text: clean,
                    previous_status_name: super::task_complete::status_name_for(
                        bob_dir,
                        row.previous_status_symbol,
                    ),
                    status_name: super::task_complete::status_name_for(
                        bob_dir,
                        row.status_symbol,
                    ),
                    unblocked_stamped: staged_has_unblocked_stamp(
                        planner,
                        bob_dir,
                        &row.note_path,
                        row.line,
                        today,
                    ),
                }
            })
            .collect();
        let row = find_offer_row(items, &verdict.key).ok_or_else(|| {
            let pull_in = super::pull_in::assemble_pull_in(items, None, &[]);
            CaptureError::offer_close_stale(
                &pull_in,
                &flat_offers(items),
                &[verdict.key.clone()],
            )
        })?;
        // The verdict's prior lane is the apply-time status: the row the
        // card showed after the gesture's recovery, not the pre-gesture
        // status the row reports for the whole gesture.
        let previous_status_name = super::task_complete::status_name_for(
            bob_dir,
            close.previous_status,
        );
        let message = verdict_message(
            verdict.outcome,
            &row.text,
            &previous_status_name,
            close.reason.as_deref(),
            &recovered,
            dry_run,
        );
        let ticklers = tickler_lines(&recovered);
        closes.push((
            close.index,
            super::output::OfferCloseJson {
                n: close.row_n,
                key: verdict.key.clone(),
                outcome: verdict.outcome.as_str(),
                reason: close.reason.clone(),
                note_path: super::dependencies::display_relative(
                    &close.note_path,
                ),
                line: row.line,
                block_id: close.block_id.clone(),
                text: row.text.clone(),
                previous_status: close.previous_status,
                previous_status_name: previous_status_name.clone(),
                cancel_log: close.cancel_log,
                recovered,
                message: message.clone(),
            },
        ));
        // Mark the resolved row: never linked, never re-offered.
        mark_row_resolved(items, &verdict.key, verdict, message, ticklers);
    }
    closes.sort_by_key(|(index, _)| *index);
    Ok(closes.into_iter().map(|(_, close)| close).collect())
}

/// Mark a resolved offer row: `selected: false`, the verdict outcome,
/// and `not_linked: resolved_in_card` with the receipt lines for human
/// output.
fn mark_row_resolved(
    items: &mut [PlannedCaptureItem],
    key: &str,
    verdict: &OfferVerdictRequest,
    message: String,
    ticklers: Vec<String>,
) {
    let outcome = verdict.outcome.as_str();
    for item in items.iter_mut() {
        super::output::for_each_offer_row_mut(&mut item.result, |row| {
            let Some(offer) = row.offer.as_mut() else {
                return;
            };
            if offer.key != key {
                return;
            }
            offer.selected = false;
            offer.verdict = Some(outcome);
            offer.verdict_message = Some(message.clone());
            offer.verdict_ticklers = ticklers.clone();
            row.not_linked = Some("resolved_in_card".to_string());
            match verdict.outcome {
                OfferVerdictOutcome::Done => {
                    row.status_symbol = 'x';
                    row.status_name = "Done".to_string();
                }
                OfferVerdictOutcome::Cancelled => {
                    row.status_symbol = '-';
                    row.status_name = "Cancelled".to_string();
                }
            }
        });
    }
}

/// Whether the staged post-recovery text stamps today on the recovered
/// row: the `[unblocked:: today]` WRITE rule, read from the bytes.
fn staged_has_unblocked_stamp(
    planner: &CaptureBatchPlanner,
    bob_dir: &Path,
    note: &Path,
    line_1based: usize,
    today: chrono::NaiveDate,
) -> bool {
    let absolute = bob_dir.join(note);
    let Some(text) = planner.peek_text(&absolute) else {
        return false;
    };
    let stamp = format!("[unblocked:: {}]", today.format("%Y-%m-%d"));
    text.lines()
        .nth(line_1based.saturating_sub(1))
        .is_some_and(|line| line.contains(&stamp))
}

/// Cancel Log outcome for a Cancel verdict: Task-Card parity — an
/// authored reason prepends (creating the log when missing); a blank
/// reason writes the `🤷` fallback only when a log already exists.
fn cancel_log_outcome(reason: &str, had_log: bool) -> &'static str {
    match (!reason.is_empty(), had_log) {
        (true, true) => "written",
        (true, false) => "created",
        (false, true) => "fallback",
        (false, false) => "none",
    }
}

fn note_has_cancel_log(source_text: &str, line_index: usize) -> bool {
    use crate::native::task_dependencies::blocks::{
        direct_child_lines, is_cancel_log,
    };
    use crate::native::task_status_hooks::{fenced_lines, logical_lines};
    let lines = logical_lines(source_text);
    if line_index >= lines.len() {
        return false;
    }
    let fenced = fenced_lines(&lines, 0..lines.len());
    direct_child_lines(&lines, &fenced, line_index)
        .into_iter()
        .any(|index| is_cancel_log(lines[index]))
}

fn snapshot_with_source(
    snapshot: &[(PathBuf, String)],
    relative: &Path,
    source_text: &str,
) -> Vec<(PathBuf, String)> {
    let mut out = snapshot.to_vec();
    match out.iter_mut().find(|(path, _)| path == relative) {
        Some((_, text)) => *text = source_text.to_string(),
        None => out.push((relative.to_path_buf(), source_text.to_string())),
    }
    out
}

/// Canonical receipt row for one verdict: the consequence sentence plus
/// the joint unblock suffix. Dry runs use the `would` forms.
fn verdict_message(
    outcome: OfferVerdictOutcome,
    text: &str,
    previous_status_name: &str,
    reason: Option<&str>,
    recovered: &[super::output::OfferCloseRecoveredJson],
    dry_run: bool,
) -> String {
    let mut message = match (outcome, dry_run) {
        (OfferVerdictOutcome::Done, false) => {
            format!("✓ completed “{text}” · {previous_status_name} → Done")
        }
        (OfferVerdictOutcome::Done, true) => {
            format!("✓ would complete “{text}” · {previous_status_name} → Done")
        }
        (OfferVerdictOutcome::Cancelled, false) => {
            format!("⊘ cancelled “{text}” · {previous_status_name} → Cancelled")
        }
        (OfferVerdictOutcome::Cancelled, true) => format!(
            "⊘ would cancel “{text}” · {previous_status_name} → Cancelled"
        ),
    };
    if matches!(outcome, OfferVerdictOutcome::Cancelled)
        && let Some(reason) = reason
        && !reason.is_empty()
    {
        message.push_str(&format!(" · “{reason}”"));
    }
    match recovered.len() {
        0 => {}
        1 => {
            let verb = if dry_run {
                "would unblock"
            } else {
                "unblocked"
            };
            message.push_str(&format!(" · {verb} “{}”", recovered[0].text));
        }
        count => {
            let verb = if dry_run {
                "would unblock"
            } else {
                "unblocked"
            };
            message.push_str(&format!(" · {verb} {count} tasks"));
        }
    }
    message
}

/// `↳` tickler receipt for dependents a verdict recovered: only stamped
/// rows come back as ticklers, with the approved singular/plural copy.
fn tickler_lines(
    recovered: &[super::output::OfferCloseRecoveredJson],
) -> Vec<String> {
    let stamped: Vec<&super::output::OfferCloseRecoveredJson> = recovered
        .iter()
        .filter(|row| row.unblocked_stamped)
        .collect();
    match stamped.len() {
        0 => Vec::new(),
        1 => vec![format!(
            "↳ “{}” back tomorrow as a tickler",
            stamped[0].text
        )],
        count => vec![format!(
            "↳ {count} unblocked tasks back tomorrow as ticklers"
        )],
    }
}

#[cfg(test)]
mod verdict_copy_tests {
    use super::*;

    fn recovered(
        text: &str,
        stamped: bool,
    ) -> super::output::OfferCloseRecoveredJson {
        super::output::OfferCloseRecoveredJson {
            note_path: "deps.md".to_string(),
            block_id: "gamma".to_string(),
            text: text.to_string(),
            previous_status_name: "Blocked".to_string(),
            status_name: "Ready".to_string(),
            unblocked_stamped: stamped,
        }
    }

    #[test]
    fn verdict_messages_use_would_forms_on_dry_runs() {
        assert_eq!(
            verdict_message(
                OfferVerdictOutcome::Done,
                "Alpha",
                "Ready",
                None,
                &[],
                false,
            ),
            "✓ completed “Alpha” · Ready → Done",
        );
        assert_eq!(
            verdict_message(
                OfferVerdictOutcome::Done,
                "Alpha",
                "Ready",
                None,
                &[],
                true,
            ),
            "✓ would complete “Alpha” · Ready → Done",
        );
        assert_eq!(
            verdict_message(
                OfferVerdictOutcome::Cancelled,
                "Beta",
                "Next",
                Some("Rerouted"),
                &[],
                false,
            ),
            "⊘ cancelled “Beta” · Next → Cancelled · “Rerouted”",
        );
        assert_eq!(
            verdict_message(
                OfferVerdictOutcome::Cancelled,
                "Beta",
                "Next",
                Some(""),
                &[],
                true,
            ),
            "⊘ would cancel “Beta” · Next → Cancelled",
        );
    }

    #[test]
    fn unblock_suffix_counts_recovered_rows() {
        let one = vec![recovered("Gamma", false)];
        assert_eq!(
            verdict_message(
                OfferVerdictOutcome::Done,
                "Alpha",
                "Ready",
                None,
                &one,
                false,
            ),
            "✓ completed “Alpha” · Ready → Done · unblocked “Gamma”",
        );
        let two = vec![recovered("Gamma", false), recovered("Delta", false)];
        assert_eq!(
            verdict_message(
                OfferVerdictOutcome::Cancelled,
                "Alpha",
                "Ready",
                None,
                &two,
                true,
            ),
            "⊘ would cancel “Alpha” · Ready → Cancelled · would unblock 2 tasks",
        );
    }

    #[test]
    fn tickler_lines_cover_only_stamped_rows() {
        assert!(tickler_lines(&[]).is_empty());
        assert!(
            tickler_lines(&[recovered("Gamma", false)]).is_empty(),
            "unstamped rows never print a tickler line",
        );
        assert_eq!(
            tickler_lines(&[recovered("Gamma", true)]),
            vec!["↳ “Gamma” back tomorrow as a tickler".to_string()],
        );
        assert_eq!(
            tickler_lines(&[
                recovered("Gamma", true),
                recovered("Delta", false),
                recovered("Epsilon", true),
            ]),
            vec!["↳ 2 unblocked tasks back tomorrow as ticklers".to_string()],
        );
    }
}

#[cfg(test)]
mod disk_guard_tests {
    use super::*;
    use std::{collections::BTreeMap, fs, path::Path};

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().expect("parent directory"))
            .expect("create directory");
        fs::write(path, contents).expect("write file");
    }

    fn snapshot(root: &Path) -> BTreeMap<std::path::PathBuf, Vec<u8>> {
        fn collect(
            root: &Path,
            directory: &Path,
            files: &mut BTreeMap<std::path::PathBuf, Vec<u8>>,
        ) {
            for entry in fs::read_dir(directory).expect("read directory") {
                let path = entry.expect("directory entry").path();
                if path.is_dir() {
                    collect(root, &path, files);
                } else {
                    let relative =
                        path.strip_prefix(root).expect("relative path");
                    files.insert(
                        relative.to_path_buf(),
                        fs::read(path).expect("read file"),
                    );
                }
            }
        }

        let mut files = BTreeMap::new();
        collect(root, root, &mut files);
        files
    }

    #[test]
    fn concurrent_verdict_target_edit_aborts_the_whole_batch() {
        // SV13: the batch's normal staged-file preimage guard must cover
        // the row the verdict post-pass closes.
        let temp = tempfile::tempdir().expect("temporary vault");
        let vault = temp.path().join("vault");
        write(
            &vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
            concat!(
                r##"{"globalFilter":"#task","statusSettings":{"coreStatuses":["##,
                r#"{"symbol":" ","name":"Ready","nextStatusSymbol":"x","type":"TODO"},"#,
                r#"{"symbol":"x","name":"Done","nextStatusSymbol":" ","type":"DONE"}],"#,
                r#""customStatuses":[{"symbol":"/","name":"In Progress","nextStatusSymbol":"x","type":"IN_PROGRESS"},"#,
                r#"{"symbol":"*","name":"Next","nextStatusSymbol":"x","type":"ON_HOLD"},"#,
                r#"{"symbol":"?","name":"Blocked","nextStatusSymbol":" ","type":"ON_HOLD"},"#,
                r#"{"symbol":"-","name":"Canceled","nextStatusSymbol":" ","type":"CANCELLED"}]}}"#,
            ),
        );
        write(&vault.join("inbox.md"), "# Inbox\n");
        write(&vault.join("sase.md"), "- [ ] #task Bar [id:: bar] ^bar\n");
        let deps_before =
            "- [?] #task Alpha [dependsOn:: bar] [id:: alpha] ^alpha\n";
        write(&vault.join("deps.md"), deps_before);
        let day_file = crate::native::pomodoro::day_file_for(&vault);
        write(
            &day_file,
            "## Pomodoros\n- [ ] (**0920-0950**) — CAPTURE\n- [ ] () — FIX\n  - [[sase#^bar]]\n",
        );
        let before = snapshot(&vault);

        let request = CaptureRequest {
            bob_dir: vault.clone(),
            dry_run: false,
            forced_clip: None,
            forced_destination_flags: Vec::new(),
            forced_route: None,
            forced_section: None,
            forced_sub_bullet_target: None,
            forced_drawer: None,
            inbox_routes: Vec::new(),
            move_in: None,
            no_clip: true,
            no_ref: true,
            pull_in: None,
            complete_offers: Vec::new(),
            cancel_offers: Vec::new(),
            offer_verdicts: vec![OfferVerdictRequest {
                key: "deps.md#^alpha".to_string(),
                outcome: OfferVerdictOutcome::Done,
                reason: String::new(),
            }],
            refuse_override: false,
            require_inbox_route_choice: false,
            require_pull_in_choice: false,
            expect_past_windows: None,
            argv: false,
            template_origin: None,
            raw_text: "!sase:bar".to_string(),
            selection: SuccessorSelection::None,
        };
        let mut batch = super::super::plan_capture_batch(&request, None)
            .expect("plan eligible Done verdict");

        let concurrent = format!("{deps_before}<!-- concurrent edit -->\n");
        write(&vault.join("deps.md"), &concurrent);
        let mut expected = before;
        expected.insert(
            std::path::PathBuf::from("deps.md"),
            concurrent.into_bytes(),
        );
        let error = super::super::commit_capture_batch(&mut batch, &vault)
            .expect_err("concurrent verdict edit refuses commit");
        assert_eq!(
            error.typed,
            Some(super::output::CaptureFailureKind::NoteChanged),
            "{error:#?}",
        );
        assert_eq!(
            snapshot(&vault),
            expected,
            "only the concurrent edit remains"
        );
    }
}

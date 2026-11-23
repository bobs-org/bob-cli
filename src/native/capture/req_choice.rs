//! Capture-side requirement card: numbering, selector split, JSON, and
//! human receipts (`docs/task-dependencies.md` §12.5).
//!
//! Normal CLI previews and `--req-choice` clients share one number space
//! with requirements first. An old client that passes
//! `--require-pull-in-choice` without `--req-choice` keeps offer-only
//! numbering, advertises `pull_in.reqs.supported`, and leaves
//! requirements at Met.

use super::*;
use crate::native::task_complete::{
    req_close::{self, groups_from_owners, ReqCloseGroup, ReqCloseOwner},
    req_verdicts::{
        resolve_verdicts, revalidate_preimages, ReqCardNumbering,
        ReqCardRowKind, ReqCardTarget, ReqVerdict, ReqVerdictError,
        ReqVerdictRequest, ReqVerdictSelection,
    },
    req_writes::ReqWritePlan,
    CompletedTask,
};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// True when this command numbers requirements first. The normal CLI
/// (neither handshake flag) is req-aware; only an old client that
/// sends `--require-pull-in-choice` alone stays on the offer-only
/// projection.
pub(super) fn is_req_aware(request: &CaptureRequest) -> bool {
    !is_legacy(request)
}

pub(super) fn is_legacy(request: &CaptureRequest) -> bool {
    request.require_pull_in_choice && !request.req_choice
}

pub(super) fn collect_owners(
    items: &[PlannedCaptureItem],
) -> Vec<ReqCloseOwner> {
    items
        .iter()
        .flat_map(|item| item.req_owners.iter().cloned())
        .collect()
}

pub(super) fn pending_req_count(owners: &[ReqCloseOwner]) -> usize {
    owners.iter().map(ReqCloseOwner::pending_count).sum()
}

/// Offer keys in the order `number_draft_offers` assigned, so
/// requirement-first numbering can append them after req rows.
pub(super) fn offer_keys_in_order(items: &[PlannedCaptureItem]) -> Vec<String> {
    let mut rows: Vec<(usize, String)> = Vec::new();
    for item in items {
        for row in super::output::all_offer_rows(&item.result) {
            if let Some(offer) = row.offer.as_ref() {
                rows.push((offer.n, offer.key.clone()));
            }
        }
    }
    rows.sort_by_key(|(n, _)| *n);
    rows.into_iter().map(|(_, key)| key).collect()
}

pub(super) fn apply_card_numbers(
    items: &mut [PlannedCaptureItem],
    numbering: &ReqCardNumbering,
) {
    let mut assigned = BTreeSet::new();
    remap_offer_ns(items, |key, n| {
        if let Ok(row) = numbering.resolve(key) {
            *n = row.n;
            assigned.insert(key.to_string());
        }
    });
    let mut next = numbering.rows.len() + 1;
    remap_offer_ns(items, |key, n| {
        if !assigned.contains(key) {
            *n = next;
            next += 1;
        }
    });
}

fn remap_offer_ns(
    items: &mut [PlannedCaptureItem],
    mut remap: impl FnMut(&str, &mut usize),
) {
    for item in items {
        for vec in super::output::unblocked_vecs_mut(&mut item.result) {
            remap_row_ns(vec, &mut remap);
        }
        for vec in super::output::relation_successors_vecs_mut(&mut item.result)
        {
            remap_row_ns(vec, &mut remap);
        }
        for vec in super::output::keep_moving_vecs_mut(&mut item.result) {
            remap_row_ns(vec, &mut remap);
        }
        for vec in super::output::note_pull_vecs_mut(&mut item.result) {
            remap_row_ns(vec, &mut remap);
        }
        remap_row_ns(
            &mut item.result.relation_merge_output.offer_rows,
            &mut remap,
        );
        for merge in &mut item.result.relation_merge_output.merges {
            for offer in &mut merge.offers {
                remap(&offer.key, &mut offer.n);
            }
        }
    }
}

fn remap_row_ns(
    rows: &mut [TaskCompleteUnblockedJson],
    remap: &mut impl FnMut(&str, &mut usize),
) {
    for row in rows {
        if let Some(offer) = row.offer.as_mut() {
            remap(&offer.key, &mut offer.n);
        }
    }
}

pub(super) fn parse_carry_args(
    raws: &[String],
) -> Result<Vec<String>, CaptureError> {
    let mut selectors = Vec::new();
    for raw in raws {
        for token in raw.split(',') {
            let token = token.trim();
            if token.is_empty() {
                return Err(CaptureError::usage(
                    "--carry has an empty token; use a comma list of requirement numbers and keys",
                ));
            }
            selectors.push(token.to_string());
        }
    }
    Ok(selectors)
}

/// Split `-k`/`-x`/`-X`/`-p`/`-j` by row kind against one card.
pub(super) struct SplitCard {
    pub req_selection: ReqVerdictSelection,
    pub offer_complete: Vec<String>,
    pub offer_cancel: Vec<(String, String)>,
    pub pull_in_keys: BTreeSet<String>,
    pub pull_in_all: bool,
    pub jump_offer: Option<String>,
}

enum CardPick {
    Requirement { key: String },
    Offer { selector: String },
}

/// Requirement row keys are `{owner}/req/{path}~{hex}` (`task_reqs::req_row_key`).
fn is_req_row_key(selector: &str) -> bool {
    selector.contains("/req/")
}

/// Map a flag selector onto a requirement row, or hand it to the offer
/// engine. `--carry` and stale `/req/` keys stay `req_close_stale`; an
/// out-of-range number or ordinary offer key keeps `offer_*_stale`.
fn classify_pick(
    numbering: &ReqCardNumbering,
    selector: &str,
    owners: &[ReqCloseOwner],
    offer_keys: &[String],
    req_only: bool,
) -> Result<CardPick, CaptureError> {
    match numbering.resolve(selector) {
        Ok(row) if row.kind == ReqCardRowKind::Requirement => {
            Ok(CardPick::Requirement {
                key: row.key.clone(),
            })
        }
        Ok(row) => {
            if req_only {
                return Err(CaptureError::usage(format!(
                    "--carry pick `{selector}` is not a requirement"
                )));
            }
            Ok(CardPick::Offer {
                selector: row.key.clone(),
            })
        }
        Err(error) => {
            if req_only || is_req_row_key(selector) {
                Err(map_req_error(error, owners, offer_keys))
            } else {
                Ok(CardPick::Offer {
                    selector: selector.to_string(),
                })
            }
        }
    }
}

pub(super) fn split_card_selection(
    owners: &[ReqCloseOwner],
    offer_keys: &[String],
    carry: &[String],
    complete: &[String],
    cancel: &[(String, String)],
    pull_in: Option<&PullInPicks>,
    jump: Option<&str>,
    req_aware: bool,
) -> Result<SplitCard, CaptureError> {
    let groups = groups_from_owners(owners);
    let numbering = if req_aware {
        ReqCardNumbering::new(&groups, offer_keys)
    } else {
        ReqCardNumbering::new(&[], offer_keys)
    }
    .map_err(|error| map_req_error(error, owners, offer_keys))?;
    let carry_numbering = if req_aware {
        numbering.clone()
    } else {
        ReqCardNumbering::new(&groups, &[])
            .map_err(|error| map_req_error(error, owners, offer_keys))?
    };

    let mut req_requests = Vec::new();
    let mut offer_complete = Vec::new();
    let mut offer_cancel = Vec::new();
    let mut pull_in_keys = BTreeSet::new();
    let mut pull_in_all = false;
    let mut req_jump: Option<String> = None;
    let mut jump_offer = None;

    for selector in carry {
        match classify_pick(
            &carry_numbering,
            selector,
            owners,
            offer_keys,
            true,
        )? {
            CardPick::Requirement { key } => {
                req_requests.push(ReqVerdictRequest {
                    selector: key,
                    verdict: ReqVerdict::Carry,
                    reason: None,
                });
            }
            CardPick::Offer { .. } => {
                return Err(CaptureError::usage(format!(
                    "--carry pick `{selector}` is not a requirement"
                )));
            }
        }
    }

    for selector in complete {
        match classify_pick(&numbering, selector, owners, offer_keys, false)? {
            CardPick::Requirement { key } => {
                req_requests.push(ReqVerdictRequest {
                    selector: key,
                    verdict: ReqVerdict::Met,
                    reason: None,
                });
            }
            CardPick::Offer { selector } => offer_complete.push(selector),
        }
    }

    for (selector, reason) in cancel {
        match classify_pick(&numbering, selector, owners, offer_keys, false)? {
            CardPick::Requirement { key } => {
                req_requests.push(ReqVerdictRequest {
                    selector: key,
                    verdict: ReqVerdict::Drop,
                    reason: (!reason.is_empty()).then(|| reason.clone()),
                });
            }
            CardPick::Offer { selector } => {
                offer_cancel.push((selector, reason.clone()));
            }
        }
    }

    match pull_in {
        None | Some(PullInPicks::None) => {}
        Some(PullInPicks::All) => {
            pull_in_all = true;
            pull_in_keys.extend(numbering.mark_all_keys());
        }
        Some(PullInPicks::Tokens { numbers, keys }) => {
            let mut selectors = Vec::new();
            for number in numbers {
                selectors.push(number.to_string());
            }
            selectors.extend(keys.iter().cloned());
            for selector in selectors {
                match classify_pick(
                    &numbering, &selector, owners, offer_keys, false,
                )? {
                    CardPick::Requirement { key } => {
                        req_requests.push(ReqVerdictRequest {
                            selector: key,
                            verdict: ReqVerdict::CarryPulled,
                            reason: None,
                        });
                    }
                    CardPick::Offer { selector } => {
                        pull_in_keys.insert(selector);
                    }
                }
            }
        }
    }

    if let Some(selector) = jump {
        match classify_pick(&numbering, selector, owners, offer_keys, false)? {
            CardPick::Requirement { key } => req_jump = Some(key),
            CardPick::Offer { selector } => jump_offer = Some(selector),
        }
    }

    let req_selection = resolve_verdicts(
        owners,
        offer_keys,
        &req_requests,
        req_jump.as_deref(),
    )
    .map_err(|error| map_req_error(error, owners, offer_keys))?;

    Ok(SplitCard {
        req_selection,
        offer_complete,
        offer_cancel,
        pull_in_keys,
        pull_in_all,
        jump_offer,
    })
}

pub(super) fn map_req_error(
    error: ReqVerdictError,
    owners: &[ReqCloseOwner],
    offer_keys: &[String],
) -> CaptureError {
    let groups = groups_from_owners(owners);
    let numbering = ReqCardNumbering::new(&groups, offer_keys).ok();
    match error {
        ReqVerdictError::Conflict { key } => CaptureError::usage_code(
            format!("conflicting requirement verdicts for `{key}`"),
            "offer_verdict_conflict",
        ),
        ReqVerdictError::Overlap {
            ancestor_text,
            descendant_key,
            ..
        } => CaptureError::usage_code(
            format!(
                "{}; conflicting pick `{descendant_key}`",
                req_close::copy::overlap(&ancestor_text)
            ),
            "req_verdict_overlap",
        ),
        ReqVerdictError::DepthExceeded { key, depth } => {
            CaptureError::usage_code(
                format!(
                    "requirement `{key}` exceeds the supported depth ({depth})"
                ),
                "req_depth_exceeded",
            )
        }
        ReqVerdictError::InvalidReason { key } => CaptureError::usage_code(
            format!("requirement `{key}` only accepts a reason on drop"),
            "offer_verdict_conflict",
        ),
        other => {
            let code = other.code();
            let selectors = match &other {
                ReqVerdictError::Stale { selectors, .. } => {
                    selectors.join(", ")
                }
                ReqVerdictError::DuplicateCardKey { key }
                | ReqVerdictError::DuplicateOwner { task_key: key } => {
                    key.clone()
                }
                _ => String::new(),
            };
            let message = format!(
                "stale, unknown, or out-of-range requirement pick: {selectors}"
            );
            let mut extra = serde_json::Map::new();
            if let Some(numbering) = numbering.as_ref() {
                extra.insert(
                    "current".to_string(),
                    serde_json::json!({
                        "groups": req_groups_json(
                            &numbering.numbered_groups(&groups).unwrap_or_else(|_| groups.clone()),
                            &BTreeSet::new(),
                        ),
                        "rows": numbering.rows.iter().map(|row| {
                            serde_json::json!({
                                "key": row.key,
                                "n": row.n,
                                "kind": match row.kind {
                                    ReqCardRowKind::Requirement => "requirement",
                                    ReqCardRowKind::TaskOffer => "task_offer",
                                },
                            })
                        }).collect::<Vec<_>>(),
                    }),
                );
            }
            CaptureError::usage_code(message, code).with_extra(extra)
        }
    }
}

pub(super) fn completed_from_owners(
    owners: &[ReqCloseOwner],
) -> Vec<CompletedTask> {
    owners
        .iter()
        .map(|owner| CompletedTask {
            relative_path: PathBuf::from(&owner.path),
            block_id: owner.block_id.clone(),
            task_id: String::new(),
            text: owner.text.clone(),
            is_root: true,
        })
        .collect()
}

/// Apply carry/drop writes and pull-in links on the final batch.
pub(super) fn plan_req_post_pass(
    planner: &mut CaptureBatchPlanner,
    request: &CaptureRequest,
    items: &[PlannedCaptureItem],
    dependency_ctx: &DependencyContext,
    today: chrono::NaiveDate,
    day_file: &Path,
    day_relative: &str,
) -> Result<
    (Vec<ReqCloseOwner>, ReqWritePlan, Option<ReqCardTarget>),
    CaptureError,
> {
    let owners = collect_owners(items);
    let jump = request
        .req_selection
        .as_ref()
        .and_then(|selection| selection.jump.clone());
    let Some(selection) = request.req_selection.as_ref() else {
        return Ok((owners, ReqWritePlan::default(), jump));
    };
    if owners.is_empty() && selection.verdicts.is_empty() {
        return Ok((owners, ReqWritePlan::default(), jump));
    }
    let offer_keys = offer_keys_in_order(items);
    revalidate_preimages(selection, &owners, &offer_keys)
        .map_err(|error| map_req_error(error, &owners, &offer_keys))?;
    let today_stamp = super::plan::date_string(today);
    let write_plan = super::req_close::plan_req_write_pass(
        planner,
        &request.bob_dir,
        &owners,
        selection,
        &today_stamp,
        today,
    )?;
    let pulled = crate::native::task_complete::req_placement::pulled_tasks(
        &owners,
        &write_plan,
    );
    if !pulled.is_empty() {
        let index = super::task_complete::staged_note_index(
            &request.bob_dir,
            planner,
            dependency_ctx,
        );
        let completed = completed_from_owners(&owners);
        for item in items {
            let item_pulled =
                crate::native::task_complete::req_placement::pulled_tasks(
                    &item.req_owners,
                    &write_plan,
                );
            if item_pulled.is_empty() {
                continue;
            }
            let forwarded;
            let closing = match item.req_closing.as_ref() {
                Some(placement) => {
                    let current =
                        planner.current_contents(day_file)?.unwrap_or_default();
                    forwarded = super::req_placement::forward_closing_lines(
                        &placement.closing,
                        &placement.day_text,
                        &current,
                    )
                    .ok_or_else(|| {
                        super::req_placement::no_destination_error(
                            item_pulled[0].row_key.clone(),
                        )
                    })?;
                    Some(&forwarded)
                }
                None => None,
            };
            super::req_placement::plan_req_link_pass(
                planner,
                &request.bob_dir,
                &super::req_placement::ReqLinkPassContext {
                    day_file,
                    day_relative: Path::new(day_relative),
                    today,
                    index: &index,
                    completed: &completed,
                    closing,
                },
                &item.req_owners,
                &write_plan,
            )?;
        }
    }
    Ok((owners, write_plan, jump))
}

/// Capability object always present on `pull_in`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PullInReqsJson {
    pub(super) supported: bool,
    pub(super) pending: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PullInReqOwnerJson {
    pub(super) task_key: String,
    pub(super) key: String,
    pub(super) path: String,
    pub(super) block_id: String,
    pub(super) line: usize,
    pub(super) text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PullInReqDetailJson {
    pub(super) owner: String,
    pub(super) path: String,
    pub(super) depth: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) parent_key: Option<String>,
    pub(super) text: String,
    pub(super) line: usize,
    pub(super) state: &'static str,
    pub(super) pending_descendants: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PullInReqConsequencesJson {
    pub(super) met: String,
    pub(super) drop: String,
    pub(super) carry: String,
    pub(super) carry_pulled: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PullInReqRowJson {
    pub(super) offer: TaskCompleteOfferJson,
    pub(super) req: PullInReqDetailJson,
    pub(super) consequences: PullInReqConsequencesJson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct ReqOutcomeJson {
    pub(super) n: usize,
    pub(super) key: String,
    pub(super) owner_key: String,
    pub(super) owner_task_key: String,
    pub(super) path: String,
    pub(super) text: String,
    pub(super) verdict: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) carried: Option<ReqCarriedJson>,
    pub(super) pulled: bool,
    pub(super) message: String,
    #[serde(skip)]
    pub(super) met_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct ReqCarriedJson {
    pub(super) path: String,
    pub(super) block_id: String,
    pub(super) line: usize,
}

pub(super) fn attach_reqs(
    pull_in: &mut PullInJson,
    items: &[PlannedCaptureItem],
    request: &CaptureRequest,
    write_plan: &ReqWritePlan,
    dry_run: bool,
) -> Vec<ReqOutcomeJson> {
    let owners = collect_owners(items);
    let pending = pending_req_count(&owners);
    pull_in.reqs = PullInReqsJson {
        supported: true,
        pending,
    };
    let req_aware = is_req_aware(request);
    let groups = groups_from_owners(&owners);
    let offer_keys = offer_keys_in_order(items);
    let numbering = if req_aware {
        ReqCardNumbering::new(&groups, &offer_keys).ok()
    } else {
        None
    };
    let numbered = numbering
        .as_ref()
        .and_then(|card| card.numbered_groups(&groups).ok())
        .unwrap_or_else(|| groups.clone());
    let pulled_keys: BTreeSet<String> = write_plan
        .outcomes
        .iter()
        .filter(|outcome| outcome.pulled)
        .map(|outcome| outcome.row_key.clone())
        .collect();
    if req_aware {
        let mut req_groups = req_groups_json(&numbered, &pulled_keys);
        req_groups.append(&mut pull_in.groups);
        pull_in.groups = req_groups;
        if let Some(card) = numbering.as_ref() {
            pull_in.offer_count = card.rows.len();
        }
        let pulled_links = write_plan
            .outcomes
            .iter()
            .filter(|outcome| outcome.pulled)
            .count();
        pull_in.picked_count += pulled_links;
    }
    req_outcomes_json(&owners, &numbered, request, write_plan, dry_run)
}

fn req_groups_json(
    groups: &[ReqCloseGroup],
    pulled_keys: &BTreeSet<String>,
) -> Vec<PullInGroupJson> {
    groups
        .iter()
        .map(|group| {
            let mut folded = PullInFoldedJson {
                hidden: 0,
                already_planned: 0,
                sub_projects: 0,
                blocked: 0,
                scheduled: 0,
                already_started: 0,
                waits_for: 0,
            };
            folded.hidden = group.folded.dropped
                + group.folded.carried
                + group.folded.met;
            PullInGroupJson {
                group: "requirements",
                kind: Some("requirements"),
                owner: Some(PullInReqOwnerJson {
                    task_key: group.owner.task_key.clone(),
                    key: group.owner.key.clone(),
                    path: group.owner.path.clone(),
                    block_id: group.owner.block_id.clone(),
                    line: group.owner.line,
                    text: group.owner.text.clone(),
                }),
                project: None,
                after: None,
                label: None,
                event: None,
                note: None,
                folded: (folded.hidden > 0).then_some(folded),
                rows: group
                    .rows
                    .iter()
                    .map(|row| PullInReqRowJson {
                        offer: TaskCompleteOfferJson {
                            key: row.offer.key.clone(),
                            n: row.offer.n,
                            group: "requirements",
                            selected: pulled_keys.contains(&row.offer.key),
                            verdict: None,
                            verdict_message: None,
                            verdict_ticklers: Vec::new(),
                            block_id_needed: false,
                            project: None,
                            after: None,
                            moved_from: None,
                            merged_from: None,
                            note: None,
                            destination:
                                super::output::TaskCompleteOfferDestinationJson {
                                    day_file: String::new(),
                                    entry_name: String::new(),
                                    entry_role: "slot",
                                    fallback: false,
                                },
                            close: None,
                            order_key: (0, (0, 0), 0, PathBuf::new(), 0),
                            settled_ordinal: None,
                        },
                        req: PullInReqDetailJson {
                            owner: row.owner_key.clone(),
                            path: row.path.clone(),
                            depth: row.depth,
                            parent_key: row.parent_key.clone(),
                            text: row.text.clone(),
                            line: row.line,
                            state: row.state,
                            pending_descendants: row.pending_descendants,
                        },
                        consequences: PullInReqConsequencesJson {
                            met: row.consequences.met.clone(),
                            drop: row.consequences.drop.clone(),
                            carry: row.consequences.carry.clone(),
                            carry_pulled: row.consequences.carry_pulled.clone(),
                        },
                    })
                    .collect(),
                copy: PullInGroupCopyJson {
                    title: group.copy.title.clone(),
                    why: Some(group.copy.why.to_string()),
                    tag: None,
                    fold: group.copy.history.clone(),
                    glyph: group.copy.glyph,
                    scope: Some(group.copy.scope.clone()),
                    history: group.copy.history.clone(),
                    footer: Some(group.copy.footer.to_string()),
                    hint: Some(group.copy.hint.to_string()),
                },
            }
        })
        .collect()
}

fn req_outcomes_json(
    owners: &[ReqCloseOwner],
    groups: &[ReqCloseGroup],
    request: &CaptureRequest,
    write_plan: &ReqWritePlan,
    dry_run: bool,
) -> Vec<ReqOutcomeJson> {
    if !write_plan.outcomes.is_empty() || !write_plan.met.is_empty() {
        let mut out = Vec::new();
        for met in &write_plan.met {
            let Some(owner) =
                owners.iter().find(|owner| owner.key == met.owner_key)
            else {
                continue;
            };
            let n = groups
                .iter()
                .find(|group| group.owner.key == owner.key)
                .and_then(|group| group.rows.first())
                .map(|row| row.offer.n)
                .unwrap_or(0);
            out.push(ReqOutcomeJson {
                n,
                key: owner.key.clone(),
                owner_key: owner.key.clone(),
                owner_task_key: owner.task_key.clone(),
                path: owner.path.clone(),
                text: owner.text.clone(),
                verdict: "met",
                reason: None,
                carried: None,
                pulled: false,
                message: if dry_run {
                    req_close::copy::would_met(met.count, &owner.text)
                } else {
                    met.receipt.clone()
                },
                met_count: met.count,
            });
        }
        for outcome in &write_plan.outcomes {
            let n = groups
                .iter()
                .flat_map(|group| group.rows.iter())
                .find(|row| row.offer.key == outcome.row_key)
                .map(|row| row.offer.n)
                .unwrap_or(0);
            out.push(ReqOutcomeJson {
                n,
                key: outcome.row_key.clone(),
                owner_key: outcome.owner_key.clone(),
                owner_task_key: outcome.owner_task_key.clone(),
                path: outcome.path.clone(),
                text: outcome.text.clone(),
                verdict: outcome.verdict.as_str(),
                reason: outcome.reason.clone(),
                carried: outcome.carried.as_ref().map(|carried| {
                    ReqCarriedJson {
                        path: carried.path.clone(),
                        block_id: carried.block_id.clone(),
                        line: carried.line,
                    }
                }),
                pulled: outcome.pulled,
                message: if dry_run {
                    would_receipt(
                        outcome.verdict,
                        &outcome.text,
                        outcome.pulled,
                        outcome.reason.as_deref(),
                    )
                } else {
                    outcome.receipt.clone()
                },
                met_count: 0,
            });
        }
        return out;
    }
    if request.req_selection.is_some() {
        return default_met_outcomes(owners, groups, dry_run);
    }
    default_met_outcomes(owners, groups, dry_run)
}

fn default_met_outcomes(
    owners: &[ReqCloseOwner],
    groups: &[ReqCloseGroup],
    dry_run: bool,
) -> Vec<ReqOutcomeJson> {
    owners
        .iter()
        .filter_map(|owner| {
            let count = owner.pending_count();
            (count > 0).then(|| {
                let n = groups
                    .iter()
                    .find(|group| group.owner.key == owner.key)
                    .and_then(|group| group.rows.first())
                    .map(|row| row.offer.n)
                    .unwrap_or(0);
                ReqOutcomeJson {
                    n,
                    key: owner.key.clone(),
                    owner_key: owner.key.clone(),
                    owner_task_key: owner.task_key.clone(),
                    path: owner.path.clone(),
                    text: owner.text.clone(),
                    verdict: "met",
                    reason: None,
                    carried: None,
                    pulled: false,
                    message: if dry_run {
                        req_close::copy::would_met(count, &owner.text)
                    } else {
                        req_close::copy::met_receipt(count, &owner.text)
                    },
                    met_count: count,
                }
            })
        })
        .collect()
}

fn would_receipt(
    verdict: ReqVerdict,
    text: &str,
    pulled: bool,
    reason: Option<&str>,
) -> String {
    match verdict {
        ReqVerdict::Drop => {
            let mut line =
                format!("would drop {}", req_close::copy::quoted(text));
            if let Some(reason) = reason.filter(|reason| !reason.is_empty()) {
                line.push_str(" · ");
                line.push_str(&req_close::copy::quoted(reason));
            }
            line
        }
        ReqVerdict::Carry | ReqVerdict::CarryPulled => {
            let mut line = format!(
                "would make {} its own task",
                req_close::copy::quoted(text)
            );
            if pulled || matches!(verdict, ReqVerdict::CarryPulled) {
                line.push_str(" · pulled in");
            }
            line
        }
        ReqVerdict::Met => req_close::copy::would_met(1, text),
    }
}

pub(super) fn print_human_req_card(
    groups: &[PullInGroupJson],
    outcomes: &[ReqOutcomeJson],
    dry_run: bool,
    styler: &Styler,
) {
    for group in groups.iter().filter(|group| group.group == "requirements") {
        println!("  {}", group.copy.title);
        if let Some(scope) = group.copy.scope.as_deref() {
            println!("  {}", styler.dim(scope));
        }
        if let Some(why) = group.copy.why.as_deref() {
            println!("  {}", styler.dim(why));
        }
        if let Some(history) = group.copy.history.as_deref() {
            println!("  {}", styler.dim(history));
        }
        for row in &group.rows {
            let indent = "  ".repeat(row.req.depth.max(1));
            // Card rows are pending by construction; depth never changes the glyph.
            let run = crate::native::task_reqs::req_glyph(
                crate::native::task_reqs::EffectiveReqState::Pending,
            );
            println!(
                "{indent}{run} {} {}",
                styler.cyan(&row.offer.n.to_string()),
                row.req.text
            );
        }
        if let Some(hint) = group.copy.hint.as_deref() {
            println!("  {}", styler.dim(hint));
        }
    }
    for outcome in outcomes {
        let printed = if outcome.verdict == "met" && !dry_run {
            req_close::copy::human_met_line(outcome.met_count, &outcome.text)
        } else {
            outcome.message.clone()
        };
        println!("  {printed}");
    }
}

/// True when Pending reqs still need an explicit acknowledgement.
pub(super) fn reqs_need_choice(
    request: &CaptureRequest,
    pending: usize,
    selection: Option<&ReqVerdictSelection>,
) -> bool {
    if !request.require_pull_in_choice || !request.req_choice || pending == 0 {
        return false;
    }
    if matches!(request.pull_in, Some(PullInPicks::None)) {
        return false;
    }
    if selection.is_some_and(|selection| !selection.verdicts.is_empty()) {
        return false;
    }
    true
}

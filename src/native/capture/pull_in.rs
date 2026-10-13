//! Capture-side successor-offer selection: parse `-p/--pull-in`, number
//! the draft-level offer list after the net pass, probe numeric picks,
//! and assemble `pull_in` JSON (`docs/task-dependencies.md` §12.5).
use super::*;
use crate::native::task_complete::SuccessorSelection;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn plan_with_pull_in(
    request: &mut CaptureRequest,
    routing: Option<&UrlRoutingPolicy>,
) -> Result<(PlannedCaptureBatch, PullInJson), CaptureError> {
    let choice = pull_in_choice(request.pull_in.as_ref());
    match request.pull_in.clone() {
        None | Some(PullInPicks::None) => {
            request.selection = SuccessorSelection::None;
            let mut batch = plan_capture_batch(request, routing)?;
            number_draft_offers(&mut batch.items, None);
            let pull_in = assemble_pull_in(&batch.items, choice);
            if request.pull_in.is_none()
                && request.require_pull_in_choice
                && pull_in.offer_count > 0
            {
                return Err(choice_required_error(&batch.items, &pull_in));
            }
            Ok((batch, pull_in))
        }
        Some(PullInPicks::All) => {
            request.selection = SuccessorSelection::All;
            let mut batch = plan_capture_batch(request, routing)?;
            number_draft_offers(&mut batch.items, None);
            let pull_in = assemble_pull_in(&batch.items, choice);
            Ok((batch, pull_in))
        }
        Some(PullInPicks::Tokens { numbers, keys }) if numbers.is_empty() => {
            let requested = keys.clone();
            request.selection =
                SuccessorSelection::Keys(keys.into_iter().collect());
            let mut batch = plan_capture_batch(request, routing)?;
            number_draft_offers(&mut batch.items, None);
            reject_unknown_keys(&batch.items, &requested)?;
            let pull_in = assemble_pull_in(&batch.items, choice);
            Ok((batch, pull_in))
        }
        Some(PullInPicks::Tokens { numbers, keys }) => {
            request.selection = SuccessorSelection::None;
            let mut probe = plan_capture_batch(request, routing)?;
            number_draft_offers(&mut probe.items, None);
            let probe_map = offer_n_map(&probe.items);
            let mut mapped = keys.clone();
            let mut bad = Vec::new();
            for number in &numbers {
                match key_for_number(&probe.items, *number) {
                    Some(key) => mapped.push(key),
                    None => bad.push(number.to_string()),
                }
            }
            if !bad.is_empty() {
                return Err(unknown_pick_error(&probe.items, &bad));
            }
            let requested = mapped.clone();
            request.selection =
                SuccessorSelection::Keys(mapped.into_iter().collect());
            let mut batch = plan_capture_batch(request, routing)?;
            number_draft_offers(&mut batch.items, Some(&probe_map));
            reject_unknown_keys(&batch.items, &requested)?;
            let pull_in = assemble_pull_in(&batch.items, choice);
            Ok((batch, pull_in))
        }
    }
}

fn pull_in_choice(picks: Option<&PullInPicks>) -> Option<&'static str> {
    match picks {
        None => None,
        Some(PullInPicks::All) => Some("all"),
        Some(PullInPicks::None) => Some("none"),
        Some(PullInPicks::Tokens { .. }) => Some("subset"),
    }
}

fn number_draft_offers(
    items: &mut [PlannedCaptureItem],
    probe_n: Option<&BTreeMap<String, usize>>,
) {
    struct RowRef {
        item: usize,
        key: String,
        order: crate::native::task_complete::OfferOrderKey,
        engine_n: usize,
    }
    let mut rows = Vec::new();
    for (item_index, item) in items.iter().enumerate() {
        for row in item_unblocked(item) {
            if let Some(offer) = row.offer.as_ref() {
                rows.push(RowRef {
                    item: item_index,
                    key: offer.key.clone(),
                    order: offer.order_key.clone(),
                    engine_n: offer.n,
                });
            }
        }
    }
    let n_map: BTreeMap<String, usize> = if let Some(probe) = probe_n {
        let mut map = probe.clone();
        let max = map.values().copied().max().unwrap_or(0);
        let mut leftover: Vec<&RowRef> = rows
            .iter()
            .filter(|row| !map.contains_key(&row.key))
            .collect();
        leftover.sort_by(|left, right| {
            left.order
                .cmp(&right.order)
                .then(left.item.cmp(&right.item))
        });
        for (index, row) in leftover.into_iter().enumerate() {
            map.insert(row.key.clone(), max + 1 + index);
        }
        map
    } else if items.len() == 1 {
        rows.iter()
            .map(|row| (row.key.clone(), row.engine_n))
            .collect()
    } else {
        rows.sort_by(|left, right| {
            left.order
                .cmp(&right.order)
                .then(left.item.cmp(&right.item))
        });
        rows.iter()
            .enumerate()
            .map(|(index, row)| (row.key.clone(), index + 1))
            .collect()
    };
    for item in items.iter_mut() {
        if let Some(rows) = item_unblocked_mut(item) {
            for row in rows {
                if let Some(offer) = row.offer.as_mut()
                    && let Some(&n) = n_map.get(&offer.key)
                {
                    offer.n = n;
                }
            }
        }
    }
}

fn offer_n_map(items: &[PlannedCaptureItem]) -> BTreeMap<String, usize> {
    let mut map = BTreeMap::new();
    for item in items {
        for row in item_unblocked(item) {
            if let Some(offer) = row.offer.as_ref() {
                map.insert(offer.key.clone(), offer.n);
            }
        }
    }
    map
}

fn key_for_number(
    items: &[PlannedCaptureItem],
    number: usize,
) -> Option<String> {
    for item in items {
        for row in item_unblocked(item) {
            if let Some(offer) = row.offer.as_ref()
                && offer.n == number
            {
                return Some(offer.key.clone());
            }
        }
    }
    None
}

fn surviving_keys(items: &[PlannedCaptureItem]) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for item in items {
        for row in item_unblocked(item) {
            if let Some(offer) = row.offer.as_ref() {
                keys.insert(offer.key.clone());
            }
        }
    }
    keys
}

fn reject_unknown_keys(
    items: &[PlannedCaptureItem],
    requested: &[String],
) -> Result<(), CaptureError> {
    let surviving = surviving_keys(items);
    let bad: Vec<String> = requested
        .iter()
        .filter(|key| !surviving.contains(*key))
        .cloned()
        .collect();
    if bad.is_empty() {
        Ok(())
    } else {
        Err(unknown_pick_error(items, &bad))
    }
}

fn unknown_pick_error(
    items: &[PlannedCaptureItem],
    bad: &[String],
) -> CaptureError {
    let mut lines = vec![format!(
        "stale, unknown, or out-of-range --pull-in pick: {}",
        bad.join(", ")
    )];
    let mut offers: Vec<(usize, String, String)> = Vec::new();
    for item in items {
        for row in item_unblocked(item) {
            if let Some(offer) = row.offer.as_ref() {
                offers.push((offer.n, offer.key.clone(), row.text.clone()));
            }
        }
    }
    offers.sort_by_key(|(n, _, _)| *n);
    for (n, key, text) in offers {
        lines.push(format!("{n} {key} {text}"));
    }
    CaptureError::usage(lines.join("\n"))
}

fn choice_required_error(
    items: &[PlannedCaptureItem],
    pull_in: &PullInJson,
) -> CaptureError {
    let offers: Vec<TaskCompleteUnblockedJson> = items
        .iter()
        .flat_map(item_unblocked)
        .filter(|row| row.offer.is_some())
        .cloned()
        .collect();
    let mut extra = serde_json::Map::new();
    extra.insert(
        "pull_in".to_string(),
        serde_json::to_value(pull_in).expect("serialize pull_in"),
    );
    extra.insert(
        "offers".to_string(),
        serde_json::to_value(&offers).expect("serialize offers"),
    );
    CaptureError::pull_in_choice_required(extra, offers)
}

pub(super) fn assemble_pull_in(
    items: &[PlannedCaptureItem],
    choice: Option<&'static str>,
) -> PullInJson {
    let mut offer_count = 0;
    let mut picked_count = 0;
    let mut folds: BTreeMap<String, crate::native::task_complete::ProjectFold> =
        BTreeMap::new();
    for item in items {
        for row in item_unblocked(item) {
            if let Some(offer) = row.offer.as_ref() {
                offer_count += 1;
                if offer.selected {
                    picked_count += 1;
                }
            }
        }
        for fold in item_project_folds(item) {
            folds.insert(display_relative(&fold.project), fold.clone());
        }
    }
    let groups = folds
        .into_iter()
        .map(|(project, fold)| PullInGroupJson {
            group: "project",
            project,
            folded: PullInFoldedJson {
                hidden: fold.hidden,
                already_planned: fold.already_planned,
                sub_projects: fold.sub_projects,
            },
        })
        .collect();
    PullInJson {
        supported: true,
        offer_count,
        choice: choice.map(str::to_string),
        picked_count,
        groups,
    }
}

pub(super) fn item_unblocked(
    item: &PlannedCaptureItem,
) -> &[TaskCompleteUnblockedJson] {
    if let Some(summary) = item.result.task_complete.as_ref() {
        &summary.unblocked
    } else if let Some(summary) = item.result.pomodoro_close.as_ref() {
        &summary.unblocked
    } else {
        &[]
    }
}

fn item_unblocked_mut(
    item: &mut PlannedCaptureItem,
) -> Option<&mut Vec<TaskCompleteUnblockedJson>> {
    if let Some(summary) = item.result.task_complete.as_mut() {
        Some(&mut summary.unblocked)
    } else if let Some(summary) = item.result.pomodoro_close.as_mut() {
        Some(&mut summary.unblocked)
    } else {
        None
    }
}

fn item_project_folds(
    item: &PlannedCaptureItem,
) -> &[crate::native::task_complete::ProjectFold] {
    if let Some(summary) = item.result.task_complete.as_ref() {
        &summary.project_folds
    } else if let Some(summary) = item.result.pomodoro_close.as_ref() {
        &summary.project_folds
    } else {
        &[]
    }
}

pub(super) fn offer_json(
    offer: &crate::native::task_complete::OfferReport,
) -> TaskCompleteOfferJson {
    use crate::native::task_complete::OfferGroup;
    TaskCompleteOfferJson {
        key: offer.key.clone(),
        n: offer.n,
        group: offer.group.as_str(),
        selected: offer.selected,
        block_id_needed: offer.block_id_needed,
        project: (offer.group == OfferGroup::Project)
            .then(|| offer.project.as_ref().map(|path| display_relative(path)))
            .flatten(),
        destination: TaskCompleteOfferDestinationJson {
            day_file: display_relative(&offer.destination.day_file),
            entry_name: offer.destination.entry_name.clone(),
            entry_role: offer.destination.entry_role.as_str(),
            fallback: offer.destination.fallback,
        },
        order_key: offer.order_key.clone(),
    }
}

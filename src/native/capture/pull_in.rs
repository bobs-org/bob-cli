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
                return Err(choice_required_error(
                    &batch.items,
                    &pull_in,
                    &batch.move_choice,
                ));
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
            // The probe exists only to map numeric picks to keys. Park
            // the inbox-route answers while it plans: they resolve
            // against the final selection's candidates, and the probe's
            // unselected batch could refuse answers the final plan
            // accepts.
            let parked_routes = std::mem::take(&mut request.inbox_routes);
            let probe_result = plan_capture_batch(request, routing);
            request.inbox_routes = parked_routes;
            let mut probe = probe_result?;
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
        for row in super::output::all_offer_rows(&item.result) {
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
    // One item with a single completion summary keeps the engine numbering
    // so old captures stay byte-identical. A single item with nested
    // relation completions gets one stable ordered number space, like a
    // multi-item draft.
    let single_summary = items.len() == 1
        && items
            .first()
            .is_some_and(|item| completion_summary_count(&item.result) <= 1);
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
    } else if single_summary {
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
        for vec in super::output::unblocked_vecs_mut(&mut item.result) {
            for row in vec.iter_mut() {
                if let Some(offer) = row.offer.as_mut()
                    && let Some(&n) = n_map.get(&offer.key)
                {
                    offer.n = n;
                }
            }
        }
        for vec in super::output::relation_successors_vecs_mut(&mut item.result)
        {
            for row in vec.iter_mut() {
                if let Some(offer) = row.offer.as_mut()
                    && let Some(&n) = n_map.get(&offer.key)
                {
                    offer.n = n;
                }
            }
        }
        for vec in super::output::keep_moving_vecs_mut(&mut item.result) {
            for row in vec.iter_mut() {
                if let Some(offer) = row.offer.as_mut()
                    && let Some(&n) = n_map.get(&offer.key)
                {
                    offer.n = n;
                }
            }
        }
        for vec in super::output::note_pull_vecs_mut(&mut item.result) {
            for row in vec.iter_mut() {
                if let Some(offer) = row.offer.as_mut()
                    && let Some(&n) = n_map.get(&offer.key)
                {
                    offer.n = n;
                }
            }
        }
        for row in &mut item.result.relation_merge_output.offer_rows {
            if let Some(offer) = row.offer.as_mut()
                && let Some(&n) = n_map.get(&offer.key)
            {
                offer.n = n;
            }
        }
        for merge in &mut item.result.relation_merge_output.merges {
            for offer in &mut merge.offers {
                if let Some(&n) = n_map.get(&offer.key) {
                    offer.n = n;
                }
            }
        }
    }
}

fn completion_summary_count(
    result: &super::output::CaptureItemResult,
) -> usize {
    let mut count = 0;
    if result.task_complete.is_some() {
        count += 1;
    }
    count += result
        .relation_merge_output
        .merges
        .iter()
        .filter(|merge| !merge.offers.is_empty())
        .count();
    count += result
        .relation_completes
        .iter()
        .filter(|complete| {
            complete.effect == super::output::RelationCompleteEffect::Completed
        })
        .count();
    if result.pomodoro_close.is_some() {
        count += 1;
    }
    if result.pomodoro_past.is_some() {
        count += 1;
    }
    if result
        .project_note
        .as_ref()
        .is_some_and(|note| note.form == Some("offer"))
    {
        count += 1;
    }
    count
}

fn offer_n_map(items: &[PlannedCaptureItem]) -> BTreeMap<String, usize> {
    let mut map = BTreeMap::new();
    for item in items {
        for row in super::output::all_offer_rows(&item.result) {
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
        for row in super::output::all_offer_rows(&item.result) {
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
        for row in super::output::all_offer_rows(&item.result) {
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
        for row in super::output::all_offer_rows(&item.result) {
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
    move_choice: &super::move_choice::MoveChoiceReport,
) -> CaptureError {
    let offers: Vec<TaskCompleteUnblockedJson> = items
        .iter()
        .flat_map(|item| super::output::all_offer_rows(&item.result))
        .filter(|row| row.offer.is_some())
        .cloned()
        .collect();
    let human_note_pull_rows = offers
        .iter()
        .filter(|row| {
            row.offer
                .as_ref()
                .is_some_and(|offer| offer.group == "note_pull")
        })
        .cloned()
        .collect();
    let human_unblocked = offers
        .iter()
        .filter(|row| {
            row.offer
                .as_ref()
                .is_some_and(|offer| offer.group != "note_pull")
        })
        .cloned()
        .collect();
    let human_note_pull_groups = pull_in
        .groups
        .iter()
        .filter(|group| group.group == "note_pull")
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
    extra.insert(
        "move_choice".to_string(),
        serde_json::to_value(move_choice).expect("serialize move_choice"),
    );
    // The refusal's human output renders keep-moving groups with their
    // folded counts, so carry the engine's folds for the offered projects.
    let folds: Vec<crate::native::task_complete::KeepMovingFold> = items
        .iter()
        .flat_map(|item| super::output::all_keep_moving_folds(&item.result))
        .cloned()
        .collect();
    let relation_groups: Vec<crate::native::task_complete::RelationOfferGroup> =
        items
            .iter()
            .flat_map(|item| super::output::all_relation_groups(&item.result))
            .cloned()
            .collect();
    let relation_held: Vec<super::output::RelationHeldJson> = items
        .iter()
        .flat_map(|item| super::output::all_relation_held(&item.result))
        .cloned()
        .collect();
    let relation_check = super::output::all_relation_check(
        items.iter().map(|item| &item.result),
    );
    CaptureError::pull_in_choice_required(
        extra,
        human_unblocked,
        folds,
        relation_groups,
        relation_held,
        relation_check,
        human_note_pull_rows,
        human_note_pull_groups,
    )
}

pub(super) fn assemble_pull_in(
    items: &[PlannedCaptureItem],
    choice: Option<&'static str>,
) -> PullInJson {
    let mut offer_count = 0;
    let mut picked_count = 0;
    for item in items {
        for row in super::output::all_offer_rows(&item.result) {
            if let Some(offer) = row.offer.as_ref() {
                offer_count += 1;
                if offer.selected {
                    picked_count += 1;
                }
            }
        }
    }
    let (groups, copy) = super::pull_in_copy::assemble_groups(items);
    PullInJson {
        supported: true,
        offer_count,
        choice: choice.map(str::to_string),
        picked_count,
        groups,
        copy,
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
        project: matches!(
            offer.group,
            OfferGroup::Project | OfferGroup::KeepMoving
        )
        .then(|| offer.project.as_ref().map(|path| display_relative(path)))
        .flatten(),
        after: (offer.group == OfferGroup::Relation)
            .then(|| {
                offer.relation.as_ref().and_then(|info| {
                    info.after.first().map(|target| {
                        format!(
                            "{}#^{}",
                            display_relative(&target.note_path),
                            target.block_id
                        )
                    })
                })
            })
            .flatten(),
        destination: TaskCompleteOfferDestinationJson {
            day_file: display_relative(&offer.destination.day_file),
            entry_name: offer.destination.entry_name.clone(),
            entry_role: offer.destination.entry_role.as_str(),
            fallback: offer.destination.fallback,
        },
        order_key: offer.order_key.clone(),
        moved_from: None,
        merged_from: offer.merged_from.clone(),
        note: None,
        close: offer.close.clone(),
        settled_ordinal: None,
    }
}

#[cfg(test)]
mod offer_json_tests {
    use super::*;
    use crate::native::task_complete::successors::{
        EntryRole, OfferDestination,
    };
    use crate::native::task_complete::{OfferGroup, OfferReport};

    fn report(merged_from: Option<&str>) -> OfferReport {
        OfferReport {
            key: "dest.md#^target".to_string(),
            n: 1,
            group: OfferGroup::HandOff,
            project: None,
            destination: OfferDestination {
                day_file: PathBuf::from("daily/2026-11-03.md"),
                entry_name: "BUILD".to_string(),
                entry_role: EntryRole::Slot,
                fallback: false,
            },
            selected: false,
            block_id_needed: false,
            merged_from: merged_from.map(str::to_string),
            order_key: (0, (0, 0), 0, PathBuf::from("dest.md"), 0),
            relation: None,
            close: None,
        }
    }

    #[test]
    fn merged_from_is_additive_and_absent_for_existing_offers() {
        let existing = serde_json::to_value(offer_json(&report(None)))
            .expect("serialize existing offer");
        assert!(existing.get("merged_from").is_none());

        let merge =
            serde_json::to_value(offer_json(&report(Some("src.md#^source"))))
                .expect("serialize merge offer");
        assert_eq!(merge["merged_from"], "src.md#^source");
    }
}

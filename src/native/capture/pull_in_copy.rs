//! Bob-authored `pull_in.groups` and top-level copy
//! (`docs/task-dependencies.md` §§12.5–12.6, 12.15, 12.18).
//!
//! Groups are assembled from surviving offers in engine order. Copy for
//! relation groups comes from `task_complete/relation_successors/copy.rs`;
//! other kinds port the picker strings from §§12.6/12.15.

use super::keep_moving_net::row_project;
use super::output::{
    all_keep_moving_folds, all_offer_rows, all_project_folds,
    all_relation_groups, all_relation_held, note_pull_source, PullInAfterJson,
    PullInFoldedJson, PullInGroupCopyJson, PullInGroupJson, PullInTopCopyJson,
    TaskCompleteUnblockedJson,
};
use super::plan::PlannedCaptureItem;
use crate::native::task_complete::relation_successors::copy::{
    self, GroupCopy,
};
use crate::native::task_complete::{
    RelationOfferGroup, PROJECT_CREATED_TRIGGER,
};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum GroupKey {
    HandOff,
    Relation { after: String, label_key: String },
    Project { project: String },
    ProjectFinished,
    KeepMoving { project: String },
    NotePull { note: String },
}

struct GroupAcc<'a> {
    key: GroupKey,
    rows: Vec<&'a TaskCompleteUnblockedJson>,
}

pub(super) fn assemble_groups(
    items: &[PlannedCaptureItem],
) -> (Vec<PullInGroupJson>, Option<PullInTopCopyJson>) {
    let mut offered = Vec::new();
    for (item_index, item) in items.iter().enumerate() {
        for row in all_offer_rows(&item.result) {
            if let Some(offer) = row.offer.as_ref() {
                offered.push((item_index, offer.order_key.clone(), row));
            }
        }
    }
    offered
        .sort_by(|left, right| left.1.cmp(&right.1).then(left.0.cmp(&right.0)));

    let mut groups: Vec<GroupAcc<'_>> = Vec::new();
    let mut index: HashMap<GroupKey, usize> = HashMap::new();
    for (_, _, row) in offered {
        let Some(key) = group_key(row) else {
            continue;
        };
        if let Some(&existing) = index.get(&key) {
            groups[existing].rows.push(row);
        } else {
            index.insert(key.clone(), groups.len());
            groups.push(GroupAcc {
                key,
                rows: vec![row],
            });
        }
    }

    let project_folds: HashMap<String, PullInFoldedJson> = items
        .iter()
        .flat_map(|item| all_project_folds(&item.result))
        .map(|fold| {
            (
                super::display_relative(&fold.project),
                PullInFoldedJson {
                    hidden: fold.hidden,
                    already_planned: fold.already_planned,
                    sub_projects: fold.sub_projects,
                    blocked: 0,
                    scheduled: 0,
                    already_started: 0,
                    waits_for: 0,
                },
            )
        })
        .collect();
    let keep_moving_folds: HashMap<String, PullInFoldedJson> = items
        .iter()
        .flat_map(|item| all_keep_moving_folds(&item.result))
        .map(|fold| {
            (
                super::display_relative(&fold.project),
                PullInFoldedJson {
                    hidden: fold.hidden,
                    already_planned: 0,
                    sub_projects: 0,
                    blocked: fold.blocked,
                    scheduled: fold.scheduled,
                    already_started: 0,
                    waits_for: 0,
                },
            )
        })
        .collect();
    let mut note_pull_folds: HashMap<String, PullInFoldedJson> = HashMap::new();
    for summary in items
        .iter()
        .filter_map(|item| item.result.note_pull.as_ref())
        .filter(|summary| {
            summary.successors.iter().any(|row| row.offer.is_some())
        })
    {
        let folded = note_pull_folds
            .entry(summary.note.clone())
            .or_insert_with(empty_folded);
        add_folded(folded, &summary.folded);
    }
    let relation_groups: Vec<&RelationOfferGroup> = items
        .iter()
        .flat_map(|item| all_relation_groups(&item.result))
        .collect();
    let held: Vec<&super::output::RelationHeldJson> = items
        .iter()
        .flat_map(|item| all_relation_held(&item.result))
        .collect();

    let json_groups: Vec<PullInGroupJson> = groups
        .iter()
        .map(|group| {
            emit_group(
                group,
                &project_folds,
                &keep_moving_folds,
                &note_pull_folds,
                &relation_groups,
                &held,
            )
        })
        .collect();
    let copy = top_copy(&json_groups, &groups, items);
    (json_groups, copy)
}

fn group_key(row: &TaskCompleteUnblockedJson) -> Option<GroupKey> {
    let offer = row.offer.as_ref()?;
    Some(match offer.group {
        "hand_off" => GroupKey::HandOff,
        "relation" => GroupKey::Relation {
            after: offer.after.clone().unwrap_or_default(),
            label_key: row
                .relation
                .as_ref()
                .map(|info| info.key.clone())
                .unwrap_or_default(),
        },
        "project" => GroupKey::Project {
            project: offer.project.clone().unwrap_or_default(),
        },
        "project_finished" => GroupKey::ProjectFinished,
        "keep_moving" => GroupKey::KeepMoving {
            project: row_project(row),
        },
        // The requested source, not the row's possibly moved home.
        "note_pull" => GroupKey::NotePull {
            note: note_pull_source(row).to_string(),
        },
        _ => return None,
    })
}

fn emit_group(
    group: &GroupAcc<'_>,
    project_folds: &HashMap<String, PullInFoldedJson>,
    keep_moving_folds: &HashMap<String, PullInFoldedJson>,
    note_pull_folds: &HashMap<String, PullInFoldedJson>,
    relation_groups: &[&RelationOfferGroup],
    held: &[&super::output::RelationHeldJson],
) -> PullInGroupJson {
    match &group.key {
        GroupKey::HandOff => PullInGroupJson {
            group: "hand_off",
            project: None,
            after: None,
            label: None,
            event: None,
            folded: None,
            note: None,
            copy: PullInGroupCopyJson {
                title: "Hand-offs".to_string(),
                why: None,
                tag: None,
                fold: None,
                glyph: "hand_off",
            },
        },
        GroupKey::Project { project } => {
            let created = group
                .rows
                .iter()
                .all(|row| row.trigger == PROJECT_CREATED_TRIGGER);
            let note = project_stem(project);
            let (title, glyph) = if created {
                (format!("New project {note}"), "project_created")
            } else {
                (format!("Project ‹{note}›"), "project")
            };
            PullInGroupJson {
                group: "project",
                project: Some(project.clone()),
                after: None,
                label: None,
                event: None,
                folded: Some(
                    project_folds
                        .get(project)
                        .cloned()
                        .unwrap_or(empty_folded()),
                ),
                note: None,
                copy: PullInGroupCopyJson {
                    title,
                    why: None,
                    tag: None,
                    fold: None,
                    glyph,
                },
            }
        }
        GroupKey::ProjectFinished => PullInGroupJson {
            group: "project_finished",
            project: None,
            after: None,
            label: None,
            event: None,
            folded: None,
            note: None,
            copy: PullInGroupCopyJson {
                title: "Project finished".to_string(),
                why: None,
                tag: None,
                fold: None,
                glyph: "project_finished",
            },
        },
        GroupKey::KeepMoving { project } => {
            let stem = project_stem(project);
            let first = group.rows.first().copied();
            let predecessors =
                first.map(|row| row.unblocked_by.as_slice()).unwrap_or(&[]);
            let why = keep_moving_why(&stem, predecessors);
            let fold = keep_moving_folds
                .get(project)
                .and_then(keep_moving_fold_line);
            PullInGroupJson {
                group: "keep_moving",
                project: Some(project.clone()),
                after: None,
                label: None,
                event: None,
                folded: Some(
                    keep_moving_folds
                        .get(project)
                        .cloned()
                        .unwrap_or(empty_folded()),
                ),
                note: None,
                copy: PullInGroupCopyJson {
                    title: format!("Keep ‹{stem}› moving"),
                    why: Some(why),
                    tag: Some(format!("ready in {stem} · not dependents")),
                    fold,
                    glyph: "keep_moving",
                },
            }
        }
        GroupKey::Relation { after, label_key } => {
            emit_relation_group(group, after, label_key, relation_groups, held)
        }
        GroupKey::NotePull { note } => {
            let stem = project_stem(note);
            let folded = note_pull_folds
                .get(note)
                .cloned()
                .unwrap_or_else(empty_folded);
            PullInGroupJson {
                group: "note_pull",
                project: None,
                after: None,
                label: None,
                event: None,
                folded: Some(folded.clone()),
                note: Some(note.clone()),
                copy: PullInGroupCopyJson {
                    title: format!("Pull from ‹{stem}›"),
                    why: None,
                    tag: Some(format!("ready in {stem}")),
                    fold: note_pull_fold_line(&folded),
                    glyph: "note_pull",
                },
            }
        }
    }
}

fn emit_relation_group(
    group: &GroupAcc<'_>,
    after_key: &str,
    label_key: &str,
    relation_groups: &[&RelationOfferGroup],
    held: &[&super::output::RelationHeldJson],
) -> PullInGroupJson {
    let first = group.rows.first().copied();
    let info = first.and_then(|row| row.relation.as_ref());
    let label = info
        .map(|info| info.label.as_str())
        .filter(|label| !label.is_empty())
        .unwrap_or(label_key);
    let event = info.map(|info| info.event).unwrap_or("parked");
    let target = info.and_then(|info| info.after.first());
    let note_path = target
        .map(|target| target.note_path.clone())
        .or_else(|| after_parts(after_key).map(|(path, _)| path))
        .unwrap_or_default();
    let block_id = target
        .map(|target| target.block_id.clone())
        .or_else(|| after_parts(after_key).map(|(_, id)| id))
        .unwrap_or_default();
    let text = target.map(|target| target.text.clone()).unwrap_or_default();
    let matched = relation_groups.iter().find(|group| {
        group.label_key == label_key && relation_after_key(group) == after_key
    });
    let copy_group = GroupCopy {
        label,
        target: &text,
    };
    let title = matched
        .map(|group| group.title.clone())
        .unwrap_or_else(|| copy::group_title(copy_group));
    let why = matched.map(|group| group.why.clone()).unwrap_or_else(|| {
        copy::event_why(
            &text,
            if event == "done" {
                copy::EventKind::Done
            } else {
                copy::EventKind::Parked
            },
        )
    });
    let tag = matched
        .map(|group| group.tag)
        .unwrap_or_else(copy::contrast_tag);
    let fold = matched
        .and_then(|group| group.fold.clone())
        .or_else(|| relation_fold_from_held(after_key, label_key, held));
    let folded = folded_if_nonzero(relation_folded(after_key, label_key, held));
    PullInGroupJson {
        group: "relation",
        project: None,
        after: Some(PullInAfterJson {
            key: after_key.to_string(),
            note_path,
            block_id,
            text,
        }),
        label: Some(label.to_string()),
        event: Some(event),
        folded,
        note: None,
        copy: PullInGroupCopyJson {
            title,
            why: Some(why),
            tag: Some(tag.to_string()),
            fold,
            glyph: "relation",
        },
    }
}

fn folded_if_nonzero(folded: PullInFoldedJson) -> Option<PullInFoldedJson> {
    let empty = folded.hidden == 0
        && folded.already_planned == 0
        && folded.sub_projects == 0
        && folded.blocked == 0
        && folded.scheduled == 0
        && folded.already_started == 0
        && folded.waits_for == 0;
    (!empty).then_some(folded)
}

fn empty_folded() -> PullInFoldedJson {
    PullInFoldedJson {
        hidden: 0,
        already_planned: 0,
        sub_projects: 0,
        blocked: 0,
        scheduled: 0,
        already_started: 0,
        waits_for: 0,
    }
}

fn add_folded(total: &mut PullInFoldedJson, folded: &PullInFoldedJson) {
    total.hidden += folded.hidden;
    total.already_planned += folded.already_planned;
    total.sub_projects += folded.sub_projects;
    total.blocked += folded.blocked;
    total.scheduled += folded.scheduled;
    total.already_started += folded.already_started;
    total.waits_for += folded.waits_for;
}

fn note_pull_fold_line(folded: &PullInFoldedJson) -> Option<String> {
    let mut parts = Vec::new();
    if folded.already_planned > 0 {
        parts.push(format!("{} already planned", folded.already_planned));
    }
    if folded.blocked > 0 {
        parts.push(format!("{} blocked", folded.blocked));
    }
    if folded.scheduled > 0 {
        parts.push(format!("{} scheduled later", folded.scheduled));
    }
    if folded.hidden > 0 {
        parts.push(format!("{} hidden", folded.hidden));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

fn relation_after_key(group: &RelationOfferGroup) -> String {
    format!(
        "{}#^{}",
        super::display_relative(&group.target.note_path),
        group.target.block_id
    )
}

fn after_parts(after: &str) -> Option<(String, String)> {
    after
        .split_once("#^")
        .map(|(path, id)| (path.to_string(), id.to_string()))
}

fn relation_folded(
    after_key: &str,
    label_key: &str,
    held: &[&super::output::RelationHeldJson],
) -> PullInFoldedJson {
    let mut folded = PullInFoldedJson {
        hidden: 0,
        already_planned: 0,
        sub_projects: 0,
        blocked: 0,
        scheduled: 0,
        already_started: 0,
        waits_for: 0,
    };
    for row in held
        .iter()
        .filter(|row| held_matches(row, after_key, label_key))
    {
        match row.reason {
            "hidden" => folded.hidden += 1,
            "already_started" => folded.already_started += 1,
            "blocked" => folded.blocked += 1,
            "waits_for" => {
                let count = row
                    .waits_for
                    .as_ref()
                    .map(Vec::len)
                    .filter(|len| *len > 0)
                    .unwrap_or(1);
                folded.waits_for += count;
            }
            "already_planned" => folded.already_planned += 1,
            _ => {}
        }
    }
    folded
}

fn relation_fold_from_held(
    after_key: &str,
    label_key: &str,
    held: &[&super::output::RelationHeldJson],
) -> Option<String> {
    let matching: Vec<_> = held
        .iter()
        .filter(|row| held_matches(row, after_key, label_key))
        .copied()
        .collect();
    let mut waits_for = Vec::new();
    let mut already_started = 0;
    let mut blocked = 0;
    let mut hidden = 0;
    let mut source = None;
    for row in &matching {
        match row.reason {
            "already_started" => already_started += 1,
            "hidden" => hidden += 1,
            "blocked" => blocked += 1,
            "waits_for" if source.is_none() => {
                source = Some(row.text.as_str());
                if let Some(targets) = row.waits_for.as_ref() {
                    waits_for.extend(
                        targets.iter().map(|target| target.text.clone()),
                    );
                }
            }
            _ => {}
        }
    }
    copy::fold_line(copy::FoldCopy {
        source,
        waits_for: &waits_for,
        already_started,
        blocked,
        hidden,
    })
}

fn held_matches(
    row: &super::output::RelationHeldJson,
    after_key: &str,
    label_key: &str,
) -> bool {
    if row.relation.key != label_key && row.relation.label != label_key {
        return false;
    }
    row.relation
        .after
        .first()
        .map(|target| {
            format!("{}#^{}", target.note_path, target.block_id) == after_key
        })
        .unwrap_or(after_key.is_empty())
}

fn keep_moving_why(
    stem: &str,
    predecessors: &[super::output::TaskCompleteUnblockedCauseJson],
) -> String {
    let first = predecessors
        .first()
        .map(|cause| copy::task_text(cause.text.trim()))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "A task".to_string());
    match predecessors.len() {
        0 | 1 => format!("✓ {first} was the last {stem} task planned today"),
        count => format!(
            "✓ {first} and {} more were the last {stem} tasks planned today",
            count - 1
        ),
    }
}

fn keep_moving_fold_line(fold: &PullInFoldedJson) -> Option<String> {
    let mut parts = Vec::new();
    if fold.blocked > 0 {
        parts.push(format!("{} blocked", fold.blocked));
    }
    if fold.scheduled > 0 {
        parts.push(format!("{} scheduled later", fold.scheduled));
    }
    if fold.hidden > 0 {
        parts.push(format!("{} hidden", fold.hidden));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

fn merge_only_copy(items: &[PlannedCaptureItem]) -> Option<PullInTopCopyJson> {
    let mut rows = Vec::new();
    for item in items {
        for row in all_offer_rows(&item.result) {
            let Some(offer) = row.offer.as_ref() else {
                continue;
            };
            rows.push(row);
            let is_merge = row.trigger == "merge"
                && offer.group == "hand_off"
                && offer.merged_from.is_some();
            if !is_merge {
                return None;
            }
        }
    }
    if rows.is_empty() {
        return None;
    }
    let mut merged: Vec<(String, String)> = Vec::new();
    for item in items {
        for record in &item.result.relation_merge_output.merges {
            if record.outcome != crate::native::task_merge::MergeOutcome::Merged
                || record.links_pruned == 0
            {
                continue;
            }
            let target = record
                .target
                .title
                .clone()
                .or_else(|| record.target.drawer.clone())
                .or_else(|| record.target.section.clone())
                .unwrap_or_default();
            merged.push((target, record.source.title.clone()));
        }
    }
    if merged.is_empty() {
        return None;
    }
    let title = if merged.len() == 1 {
        format!("Link “{}” in place of “{}”", merged[0].0, merged[0].1)
    } else {
        format!(
            "Link “{}” in place of {} merged tasks",
            merged[0].0,
            merged.len()
        )
    };
    Some(PullInTopCopyJson {
        title,
        capsule: "Pull in".to_string(),
        glyph: "hand_off",
    })
}

fn top_copy(
    groups: &[PullInGroupJson],
    acc: &[GroupAcc<'_>],
    items: &[PlannedCaptureItem],
) -> Option<PullInTopCopyJson> {
    if groups.is_empty() {
        return None;
    }
    if let Some(merge_copy) = merge_only_copy(items) {
        return Some(merge_copy);
    }
    let relation_only = groups.iter().all(|group| group.group == "relation");
    let keep_only = groups.iter().all(|group| group.group == "keep_moving");
    let note_pull_only = groups.iter().all(|group| group.group == "note_pull");
    if note_pull_only {
        let notes: Vec<&str> = groups
            .iter()
            .filter_map(|group| group.note.as_deref())
            .collect();
        let title = if notes.len() == 1 {
            format!("Pull from ‹{}›", project_stem(notes[0]))
        } else {
            format!("Pull from {} notes", notes.len())
        };
        return Some(PullInTopCopyJson {
            title,
            capsule: "Pull in".to_string(),
            glyph: "note_pull",
        });
    }
    if relation_only {
        return Some(relation_top_copy(groups, acc));
    }
    if keep_only {
        return Some(keep_moving_top_copy(groups));
    }
    let predecessors = mixed_predecessors(acc);
    let title = if predecessors.len() == 1 {
        format!("Pull in successors of {}", predecessors[0])
    } else if !predecessors.is_empty() {
        format!("Pull in successors ({} closed)", predecessors.len())
    } else {
        groups[0].copy.title.clone()
    };
    Some(PullInTopCopyJson {
        title,
        capsule: "Pull in".to_string(),
        glyph: groups[0].copy.glyph,
    })
}

fn relation_top_copy(
    groups: &[PullInGroupJson],
    acc: &[GroupAcc<'_>],
) -> PullInTopCopyJson {
    let first = &groups[0];
    let label = first.label.as_deref().unwrap_or("starts after");
    let target = first
        .after
        .as_ref()
        .map(|after| after.text.as_str())
        .unwrap_or_default();
    let row_count: usize = acc
        .iter()
        .filter(|group| matches!(group.key, GroupKey::Relation { .. }))
        .map(|group| group.rows.len())
        .sum();
    let title = if groups.len() == 1 && !target.is_empty() {
        copy::group_title(GroupCopy { label, target })
    } else {
        copy::picker_title(GroupCopy { label, target }, Some(row_count), None)
    };
    PullInTopCopyJson {
        title,
        capsule: copy::capsule(label),
        glyph: "relation",
    }
}

fn keep_moving_top_copy(groups: &[PullInGroupJson]) -> PullInTopCopyJson {
    let stems: Vec<String> = groups
        .iter()
        .filter_map(|group| group.project.as_deref().map(project_stem))
        .collect();
    let title = if stems.len() == 1 {
        format!("Keep ‹{}› moving", stems[0])
    } else {
        format!("Keep {} projects moving", stems.len())
    };
    PullInTopCopyJson {
        title,
        capsule: "Keep moving".to_string(),
        glyph: "keep_moving",
    }
}

fn mixed_predecessors(acc: &[GroupAcc<'_>]) -> Vec<String> {
    let mut seen = Vec::new();
    for group in acc {
        for row in &group.rows {
            for cause in &row.unblocked_by {
                let text = copy::task_text(cause.text.trim());
                if text.is_empty()
                    || seen.iter().any(|existing| existing == &text)
                {
                    continue;
                }
                seen.push(text);
            }
        }
    }
    seen
}

fn project_stem(project: &str) -> String {
    Path::new(project)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| project.to_string())
}

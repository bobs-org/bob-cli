//! Pure relation-successor extraction and R1–R7 classification.
//!
//! The successor planner merges this read-only result after recovery.
//! This module never writes the vault or enumerates notes beyond the
//! source texts and narrow target reads the caller supplies.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use super::copy::{self, FoldCopy, GroupCopy};
use super::{
    RelationEvent, RelationEventKind, RelationHeldReason, RelationHeldRow,
    RelationTarget,
};
use crate::native::capture_dependency_tasks::{
    display_path, has_hide_tag, has_prj_is_blocked_tag,
};
use crate::native::config::relations::CancelLabel;
use crate::native::note_tasks::stable_task_key;
use crate::native::relation_labels::relation_label_key;
use crate::native::task_complete::successors::{raw_task_line, DayView};
use crate::native::task_dependencies::is_archive_path;
use crate::native::task_relations::parse::links_children_of;
use crate::native::task_relations::resolve::resolve_relation_target_path;
use crate::native::task_relations::{LinksLine, RelationEntry};
use crate::native::task_status_hooks::{
    fenced_lines, logical_lines, parse_tasks, FileScan, TaskLine,
    TaskStatusType, TasksSettings,
};
use crate::native::vault_links::NoteIndex;

/// Narrow target-note read used after the cheap gates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TargetNoteRead {
    Text(String),
    Missing,
    #[cfg_attr(not(test), allow(dead_code))] // Tests and the integrate phase.
    Unavailable,
}

pub(crate) trait TargetNoteReader {
    fn read_target(&mut self, path: &Path) -> TargetNoteRead;
}

/// Caller-supplied post-recovery overlay for R4.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SourceRecovery {
    pub recovered_prerequisites: bool,
    pub recovered_project: bool,
    pub project_path: Option<PathBuf>,
    pub open_prerequisite_count: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RelationCheck {
    Checked,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RelationOfferRow {
    pub source: RelationTarget,
    pub source_line: usize,
    pub lane: &'static str,
    pub triggering_targets: Vec<RelationTarget>,
    pub suffix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RelationOfferGroup {
    pub target: RelationTarget,
    pub target_line: usize,
    pub label: String,
    pub label_key: String,
    pub event: RelationEventKind,
    pub rows: Vec<RelationOfferRow>,
    pub title: String,
    pub why: String,
    pub tag: &'static str,
    pub fold: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RelationClassifyResult {
    pub groups: Vec<RelationOfferGroup>,
    pub held: Vec<RelationHeldRow>,
    pub check: Option<RelationCheck>,
    pub target_reads: usize,
}

pub(crate) struct RelationClassifyInput<'a> {
    pub labels: &'a [CancelLabel],
    pub events: &'a [RelationEvent],
    pub source_texts: &'a BTreeMap<PathBuf, String>,
    pub known_paths: &'a [PathBuf],
    pub archive_paths: &'a [PathBuf],
    pub day_before: Option<&'a str>,
    pub day_after: Option<&'a str>,
    pub day_path: &'a Path,
    pub previous_daily: Option<&'a Path>,
    pub today: NaiveDate,
    pub link_unblocked: bool,
    pub snapshot_complete: bool,
    pub dependency_snapshot_complete: bool,
    pub recovered: &'a BTreeMap<String, SourceRecovery>,
    pub tasks_settings: &'a TasksSettings,
}

struct NoteCache<'a> {
    staged: &'a BTreeMap<PathBuf, String>,
    loaded: BTreeMap<PathBuf, TargetNoteRead>,
    reads: usize,
}

impl<'a> NoteCache<'a> {
    fn new(staged: &'a BTreeMap<PathBuf, String>) -> Self {
        Self {
            staged,
            loaded: BTreeMap::new(),
            reads: 0,
        }
    }

    fn load<R: TargetNoteReader>(
        &mut self,
        path: &Path,
        reader: &mut R,
    ) -> TargetNoteRead {
        if let Some(text) = self.staged.get(path) {
            return TargetNoteRead::Text(text.clone());
        }
        if let Some(existing) = self.loaded.get(path) {
            return existing.clone();
        }
        self.reads += 1;
        let read = reader.read_target(path);
        self.loaded.insert(path.to_path_buf(), read.clone());
        read
    }
}

struct SourceHit {
    path: PathBuf,
    task: TaskLine,
    raw: String,
    edges: Vec<ResolvedEdge>,
}

struct ResolvedEdge {
    label_key: String,
    target_path: PathBuf,
    target_id: String,
}

struct ValidEvent {
    event: RelationEvent,
    target_line: usize,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct GroupKey {
    order: (usize, usize),
    path: PathBuf,
    line: usize,
    label_key: String,
}

struct TriggeringEvent {
    event: RelationEvent,
    target_line: usize,
    label_key: String,
    order: (usize, usize),
}

enum SourceClass {
    Skip,
    Held(RelationHeldReason),
    Offer { lane: &'static str },
}

struct HeldForFold {
    source_text: String,
    reason: RelationHeldReason,
}

/// Classify relation successors over in-memory source texts and a
/// narrow target reader. The integrate phase merges this result.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn classify_relation_successors<R: TargetNoteReader>(
    input: &RelationClassifyInput<'_>,
    reader: &mut R,
) -> RelationClassifyResult {
    if input.labels.is_empty()
        || !input.link_unblocked
        || input.events.is_empty()
        || input
            .events
            .iter()
            .all(|event| event.target.block_id.is_empty())
    {
        return RelationClassifyResult::default();
    }
    if !input.snapshot_complete {
        return RelationClassifyResult {
            check: Some(RelationCheck::Unavailable),
            ..RelationClassifyResult::default()
        };
    }

    let notes = NoteIndex::from_paths(input.known_paths.iter().cloned());
    let baseline = input
        .day_before
        .map(|text| DayView::parse(text, input.day_path, &notes));
    let post = input
        .day_after
        .map(|text| DayView::parse(text, input.day_path, &notes));
    let mut cache = NoteCache::new(input.source_texts);

    let valid = valid_events(
        input,
        &mut cache,
        reader,
        baseline.as_ref(),
        post.as_ref(),
    );
    if valid.is_empty() {
        return RelationClassifyResult {
            target_reads: cache.reads,
            ..RelationClassifyResult::default()
        };
    }

    let sources = extract_sources(input, &notes, &mut cache, reader);
    let event_keys = valid
        .iter()
        .map(|event| {
            identity_key(
                &event.event.target.note_path,
                &event.event.target.block_id,
            )
        })
        .collect::<BTreeSet<_>>();
    let mut hits = sources
        .into_iter()
        .filter(|source| {
            source.edges.iter().any(|edge| {
                event_keys
                    .contains(&identity_key(&edge.target_path, &edge.target_id))
            })
        })
        .collect::<Vec<_>>();
    if hits.is_empty() {
        return RelationClassifyResult {
            target_reads: cache.reads,
            ..RelationClassifyResult::default()
        };
    }

    let group_order = group_sort_keys(&valid, baseline.as_ref());
    let mut offers: BTreeMap<GroupKey, Vec<RelationOfferRow>> = BTreeMap::new();
    let mut held = Vec::new();
    let mut held_for_fold: BTreeMap<GroupKey, Vec<HeldForFold>> =
        BTreeMap::new();

    hits.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.task.line_index.cmp(&right.task.line_index))
    });

    for source in hits {
        let triggering = triggering_for(&source, &valid, &group_order);
        if triggering.is_empty() {
            continue;
        }
        let first = &triggering[0];
        let group_key = GroupKey {
            order: first.order,
            path: first.event.target.note_path.clone(),
            line: first.target_line,
            label_key: first.label_key.clone(),
        };
        let source_target = relation_target(
            &source.path,
            source.task.block_id.as_deref(),
            &source.task.description,
            source.task.line_index,
            input.tasks_settings,
        );
        let triggering_targets = triggering
            .iter()
            .map(|item| item.event.target.clone())
            .collect::<Vec<_>>();
        match classify_source(
            input,
            &source,
            &triggering,
            &event_keys,
            &mut cache,
            reader,
            post.as_ref(),
            baseline.as_ref(),
        ) {
            SourceClass::Skip => {}
            SourceClass::Held(reason) => {
                held_for_fold
                    .entry(group_key)
                    .or_default()
                    .push(HeldForFold {
                        source_text: source_target.text.clone(),
                        reason: reason.clone(),
                    });
                held.push(RelationHeldRow {
                    source: source_target,
                    reason,
                    triggering_targets,
                });
            }
            SourceClass::Offer { lane } => {
                let texts = triggering_targets
                    .iter()
                    .map(|target| target.text.clone())
                    .collect::<Vec<_>>();
                offers.entry(group_key).or_default().push(RelationOfferRow {
                    source: source_target,
                    source_line: source.task.line_index,
                    lane,
                    triggering_targets,
                    suffix: copy::trigger_suffix(&texts),
                });
            }
        }
    }

    let mut groups = Vec::new();
    for (key, mut rows) in offers {
        rows.sort_by(|left, right| {
            lane_order(left.lane)
                .cmp(&lane_order(right.lane))
                .then(left.source.note_path.cmp(&right.source.note_path))
                .then(left.source_line.cmp(&right.source_line))
        });
        let Some(event) = valid.iter().find(|item| {
            item.event.target.note_path == key.path
                && item.target_line == key.line
        }) else {
            continue;
        };
        groups.push(build_group(
            event,
            &key,
            input.labels,
            rows,
            held_for_fold.remove(&key).unwrap_or_default(),
        ));
    }
    groups.sort_by(|left, right| {
        group_order_of(
            &group_order,
            &left.target,
            left.target_line,
            &left.label_key,
        )
        .cmp(&group_order_of(
            &group_order,
            &right.target,
            right.target_line,
            &right.label_key,
        ))
    });

    RelationClassifyResult {
        groups,
        held,
        check: Some(RelationCheck::Checked),
        target_reads: cache.reads,
    }
}

fn valid_events<R: TargetNoteReader>(
    input: &RelationClassifyInput<'_>,
    cache: &mut NoteCache<'_>,
    reader: &mut R,
    baseline: Option<&DayView>,
    post: Option<&DayView>,
) -> Vec<ValidEvent> {
    let mut valid = Vec::new();
    for event in input.events {
        if event.target.block_id.is_empty() {
            continue;
        }
        let loaded = load_task(
            input,
            cache,
            reader,
            &event.target.note_path,
            &event.target.block_id,
        );
        match event.kind {
            RelationEventKind::Done => {
                valid.push(ValidEvent {
                    event: event.clone(),
                    target_line: loaded
                        .map(|task| task.line_index)
                        .unwrap_or(0),
                });
            }
            RelationEventKind::Parked => {
                let Some(task) = loaded else {
                    continue;
                };
                let key = (
                    event.target.note_path.clone(),
                    event.target.block_id.clone(),
                );
                let baseline_live =
                    baseline.is_some_and(|view| view.live.contains_key(&key));
                let post_live =
                    post.is_some_and(|view| view.live.contains_key(&key));
                if task.status == '/' && baseline_live && !post_live {
                    valid.push(ValidEvent {
                        event: event.clone(),
                        target_line: task.line_index,
                    });
                }
            }
        }
    }
    valid
}

fn extract_sources<R: TargetNoteReader>(
    input: &RelationClassifyInput<'_>,
    notes: &NoteIndex,
    cache: &mut NoteCache<'_>,
    reader: &mut R,
) -> Vec<SourceHit> {
    let keys = input
        .labels
        .iter()
        .map(|label| label.key.clone())
        .collect::<BTreeSet<_>>();
    let mut sources = Vec::new();
    for (path, contents) in input.source_texts {
        let scan = FileScan {
            path: path.clone(),
            relative_path: path.clone(),
            contents: contents.clone(),
            tasks: parse_tasks(contents, input.tasks_settings),
        };
        let lines = logical_lines(&scan.contents);
        let fenced = fenced_lines(&lines, 0..lines.len());
        for task in &scan.tasks {
            let mut edges = Vec::new();
            for child in links_children_of(&lines, &fenced, task.line_index) {
                let LinksLine::Accepted { entries, .. } = child.parsed else {
                    continue;
                };
                for entry in entries {
                    if let Some(edge) = resolve_edge(
                        path, task, &entry, &keys, notes, input, cache, reader,
                    ) {
                        edges.push(edge);
                    }
                }
            }
            if edges.is_empty() {
                continue;
            }
            sources.push(SourceHit {
                path: path.clone(),
                task: task.clone(),
                raw: raw_task_line(&scan.contents, task.line_index),
                edges,
            });
        }
    }
    sources
}

fn resolve_edge<R: TargetNoteReader>(
    source_path: &Path,
    source: &TaskLine,
    entry: &RelationEntry,
    keys: &BTreeSet<String>,
    notes: &NoteIndex,
    input: &RelationClassifyInput<'_>,
    cache: &mut NoteCache<'_>,
    reader: &mut R,
) -> Option<ResolvedEdge> {
    let label = entry.label.as_deref()?;
    let key = relation_label_key(label);
    if !keys.contains(&key) {
        return None;
    }
    let target_path = resolve_relation_target_path(
        source_path,
        &entry.target,
        notes,
        |path| input.archive_paths.iter().any(|known| known == path),
    )?;
    if source.block_id.as_deref() == Some(entry.block_id.as_str())
        && target_path == source_path
    {
        return None;
    }
    let edge = ResolvedEdge {
        label_key: key,
        target_path: target_path.clone(),
        target_id: entry.block_id.clone(),
    };
    match cache.load(&target_path, reader) {
        // A resolved path we cannot read is not released (R6).
        TargetNoteRead::Unavailable => Some(edge),
        TargetNoteRead::Missing => None,
        TargetNoteRead::Text(contents) => {
            parse_tasks(&contents, input.tasks_settings)
                .iter()
                .any(|task| {
                    task.block_id.as_deref() == Some(entry.block_id.as_str())
                })
                .then_some(edge)
        }
    }
}

fn triggering_for(
    source: &SourceHit,
    events: &[ValidEvent],
    group_order: &[((usize, usize), RelationEvent)],
) -> Vec<TriggeringEvent> {
    let mut triggering = Vec::new();
    for edge in &source.edges {
        for item in events {
            if item.event.target.note_path == edge.target_path
                && item.event.target.block_id == edge.target_id
            {
                let order = group_order
                    .iter()
                    .find(|(_, event)| {
                        event.target.note_path == item.event.target.note_path
                            && event.target.block_id
                                == item.event.target.block_id
                    })
                    .map(|(order, _)| *order)
                    .unwrap_or((usize::MAX, usize::MAX));
                triggering.push(TriggeringEvent {
                    event: item.event.clone(),
                    target_line: item.target_line,
                    label_key: edge.label_key.clone(),
                    order,
                });
            }
        }
    }
    triggering.sort_by(|left, right| {
        left.order
            .cmp(&right.order)
            .then(
                left.event
                    .target
                    .note_path
                    .cmp(&right.event.target.note_path),
            )
            .then(left.target_line.cmp(&right.target_line))
            .then(left.label_key.cmp(&right.label_key))
    });
    triggering.dedup_by(|left, right| {
        left.event.target == right.event.target
            && left.label_key == right.label_key
    });
    triggering
}

fn classify_source<R: TargetNoteReader>(
    input: &RelationClassifyInput<'_>,
    source: &SourceHit,
    triggering: &[TriggeringEvent],
    event_keys: &BTreeSet<String>,
    cache: &mut NoteCache<'_>,
    reader: &mut R,
    post: Option<&DayView>,
    baseline: Option<&DayView>,
) -> SourceClass {
    let key = source_identity_key(
        &source.path,
        source.task.block_id.as_deref(),
        source.task.line_index,
    );
    let recovery = input.recovered.get(&key).cloned().unwrap_or_else(|| {
        // Legacy callers keyed missing-ID tasks by bare path; fall back
        // so older overlays still apply.
        input
            .recovered
            .get(&identity_key(
                &source.path,
                source.task.block_id.as_deref().unwrap_or(""),
            ))
            .cloned()
            .unwrap_or_default()
    });

    if is_archive_path(&source.path)
        || input
            .previous_daily
            .is_some_and(|path| path == source.path.as_path())
        || !source.task.status_type.is_open()
        || matches!(source.task.status, 'x' | 'X' | '-')
    {
        return SourceClass::Skip;
    }

    let source_live_key = source
        .task
        .block_id
        .as_ref()
        .map(|id| (source.path.clone(), id.clone()));
    let parked_here = triggering.iter().any(|item| {
        matches!(item.event.kind, RelationEventKind::Parked)
            && item.event.target.note_path == source.path
            && source.task.block_id.as_deref()
                == Some(item.event.target.block_id.as_str())
    });
    let baseline_live = source_live_key.as_ref().is_some_and(|key| {
        baseline.is_some_and(|view| view.live.contains_key(key))
    });
    if parked_here || baseline_live {
        return SourceClass::Held(RelationHeldReason::AlreadyPlanned);
    }
    if source.task.status == '/' {
        return SourceClass::Held(RelationHeldReason::AlreadyStarted);
    }
    if let Some(reason) = blocked_reason(input, source, &recovery) {
        return SourceClass::Held(reason);
    }
    if has_hide_tag(&source.raw) {
        return SourceClass::Held(RelationHeldReason::Hidden);
    }
    if let Some(waiting) =
        waits_for(input, source, event_keys, cache, reader, post)
    {
        return SourceClass::Held(RelationHeldReason::WaitsFor(waiting));
    }
    match source.task.status {
        '*' => SourceClass::Offer { lane: "Next" },
        ' ' => SourceClass::Offer { lane: "Ready" },
        '?' if recovery.recovered_prerequisites => {
            SourceClass::Offer { lane: "Ready" }
        }
        _ => SourceClass::Skip,
    }
}

fn blocked_reason(
    input: &RelationClassifyInput<'_>,
    source: &SourceHit,
    recovery: &SourceRecovery,
) -> Option<RelationHeldReason> {
    let mut waits_on = Vec::new();
    let mut scheduled = None;
    let mut project = recovery.project_path.clone();
    let mut blocked = false;

    let prereq_count = recovery
        .open_prerequisite_count
        .unwrap_or(source.task.depends_on.len());
    let deps_unknown = !source.task.depends_on.is_empty()
        && !input.dependency_snapshot_complete
        && !recovery.recovered_prerequisites;
    let open_prereqs = !recovery.recovered_prerequisites
        && (source.task.status == '?' || prereq_count > 0 || deps_unknown);
    if open_prereqs {
        blocked = true;
        let count = prereq_count.max(1);
        waits_on = (0..count)
            .map(|_| RelationTarget {
                note_path: PathBuf::new(),
                block_id: String::new(),
                text: String::new(),
            })
            .collect();
    }
    if source.task.scheduled.is_some_and(|date| date > input.today) {
        blocked = true;
        scheduled = source.task.scheduled.map(|date| date.to_string());
    }
    let project_blocked =
        has_prj_is_blocked_tag(&source.raw) && !recovery.recovered_project;
    if project_blocked {
        blocked = true;
        if project.is_none() {
            project = Some(source.path.clone());
        }
    }
    blocked.then_some(RelationHeldReason::Blocked {
        waits_on,
        scheduled,
        project,
    })
}

fn waits_for<R: TargetNoteReader>(
    input: &RelationClassifyInput<'_>,
    source: &SourceHit,
    event_keys: &BTreeSet<String>,
    cache: &mut NoteCache<'_>,
    reader: &mut R,
    post: Option<&DayView>,
) -> Option<Vec<RelationTarget>> {
    let mut waiting = Vec::new();
    let mut seen = BTreeSet::new();
    for edge in &source.edges {
        let key = identity_key(&edge.target_path, &edge.target_id);
        if event_keys.contains(&key) || !seen.insert(key) {
            continue;
        }
        match released(
            input,
            &edge.target_path,
            &edge.target_id,
            cache,
            reader,
            post,
        ) {
            Release::Yes => {}
            Release::No { target } => waiting.push(target),
            Release::Unresolved => {}
        }
    }
    (!waiting.is_empty()).then_some(waiting)
}

enum Release {
    Yes,
    No { target: RelationTarget },
    Unresolved,
}

fn released<R: TargetNoteReader>(
    input: &RelationClassifyInput<'_>,
    path: &Path,
    block_id: &str,
    cache: &mut NoteCache<'_>,
    reader: &mut R,
    post: Option<&DayView>,
) -> Release {
    let read = cache.load(path, reader);
    if matches!(read, TargetNoteRead::Unavailable) {
        return Release::No {
            target: relation_target(
                path,
                Some(block_id),
                "",
                0,
                input.tasks_settings,
            ),
        };
    }
    let Some(task) = load_task(input, cache, reader, path, block_id) else {
        return Release::Unresolved;
    };
    let live = post.is_some_and(|view| {
        view.live
            .contains_key(&(path.to_path_buf(), block_id.to_string()))
    });
    let done = matches!(
        task.status_type,
        TaskStatusType::Done | TaskStatusType::Cancelled
    ) || matches!(task.status, 'x' | 'X' | '-');
    let parked = task.status == '/' && !live;
    if done || parked {
        Release::Yes
    } else {
        Release::No {
            target: relation_target(
                path,
                Some(block_id),
                &task.description,
                task.line_index,
                input.tasks_settings,
            ),
        }
    }
}

fn load_task<R: TargetNoteReader>(
    input: &RelationClassifyInput<'_>,
    cache: &mut NoteCache<'_>,
    reader: &mut R,
    path: &Path,
    block_id: &str,
) -> Option<TaskLine> {
    let contents = match cache.load(path, reader) {
        TargetNoteRead::Text(text) => text,
        TargetNoteRead::Missing | TargetNoteRead::Unavailable => return None,
    };
    parse_tasks(&contents, input.tasks_settings)
        .into_iter()
        .find(|task| task.block_id.as_deref() == Some(block_id))
}

fn group_sort_keys(
    events: &[ValidEvent],
    baseline: Option<&DayView>,
) -> Vec<((usize, usize), RelationEvent)> {
    let mut keys = events
        .iter()
        .map(|item| {
            let live = baseline.and_then(|view| {
                view.live.get(&(
                    item.event.target.note_path.clone(),
                    item.event.target.block_id.clone(),
                ))
            });
            let order = live
                .map(|(entry, line, _, _)| (*entry, *line))
                .unwrap_or((usize::MAX, usize::MAX));
            (order, item.event.clone(), item.target_line)
        })
        .collect::<Vec<_>>();
    keys.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then(left.1.target.note_path.cmp(&right.1.target.note_path))
            .then(left.2.cmp(&right.2))
            .then(left.1.target.block_id.cmp(&right.1.target.block_id))
    });
    keys.into_iter()
        .map(|(order, event, _)| (order, event))
        .collect()
}

fn group_order_of(
    group_order: &[((usize, usize), RelationEvent)],
    target: &RelationTarget,
    target_line: usize,
    label_key: &str,
) -> (usize, usize, PathBuf, usize, String) {
    let order = group_order
        .iter()
        .find(|(_, event)| {
            event.target.note_path == target.note_path
                && event.target.block_id == target.block_id
        })
        .map(|(order, _)| *order)
        .unwrap_or((usize::MAX, usize::MAX));
    (
        order.0,
        order.1,
        target.note_path.clone(),
        target_line,
        label_key.to_string(),
    )
}

fn build_group(
    event: &ValidEvent,
    key: &GroupKey,
    labels: &[CancelLabel],
    rows: Vec<RelationOfferRow>,
    held: Vec<HeldForFold>,
) -> RelationOfferGroup {
    let label = labels
        .iter()
        .find(|label| label.key == key.label_key)
        .map(|label| label.name.clone())
        .unwrap_or_else(|| key.label_key.clone());
    let copy = GroupCopy {
        label: &label,
        target: &event.event.target.text,
    };
    let mut waits_for = Vec::new();
    let mut already_started = 0;
    let mut blocked = 0;
    let mut hidden = 0;
    let mut wait_source = None;
    for item in &held {
        match &item.reason {
            RelationHeldReason::AlreadyStarted => already_started += 1,
            RelationHeldReason::Hidden => hidden += 1,
            RelationHeldReason::Blocked { .. }
            | RelationHeldReason::Scheduled(_)
            | RelationHeldReason::Project => blocked += 1,
            RelationHeldReason::WaitsFor(targets) => {
                if wait_source.is_none() {
                    wait_source = Some(item.source_text.clone());
                    waits_for.extend(
                        targets.iter().map(|target| target.text.clone()),
                    );
                }
            }
            RelationHeldReason::AlreadyPlanned => {}
        }
    }
    RelationOfferGroup {
        target: event.event.target.clone(),
        target_line: event.target_line,
        label: label.clone(),
        label_key: key.label_key.clone(),
        event: event.event.kind,
        rows,
        title: copy::group_title(copy),
        why: copy::event_why(&event.event.target.text, event.event.kind),
        tag: copy::contrast_tag(),
        fold: copy::fold_line(FoldCopy {
            source: wait_source.as_deref(),
            waits_for: &waits_for,
            already_started,
            blocked,
            hidden,
        }),
    }
}

fn relation_target(
    path: &Path,
    block_id: Option<&str>,
    description: &str,
    line_index: usize,
    settings: &TasksSettings,
) -> RelationTarget {
    let id = block_id.map(str::to_string).unwrap_or_else(|| {
        stable_task_key(
            path,
            None,
            description,
            &settings.global_filter,
            line_index,
        )
    });
    RelationTarget {
        note_path: path.to_path_buf(),
        block_id: id,
        text: description.to_string(),
    }
}

fn identity_key(path: &Path, block_id: &str) -> String {
    if block_id.is_empty() {
        display_path(path)
    } else {
        format!("{}#^{}", display_path(path), block_id)
    }
}

fn source_identity_key(
    path: &Path,
    block_id: Option<&str>,
    line: usize,
) -> String {
    match block_id {
        Some(id) if !id.is_empty() => identity_key(path, id),
        _ => format!("{}@{line}", display_path(path)),
    }
}

fn lane_order(lane: &str) -> u8 {
    match lane {
        "Next" => 0,
        "Ready" => 1,
        _ => 2,
    }
}

impl TargetNoteReader for BTreeMap<PathBuf, String> {
    fn read_target(&mut self, path: &Path) -> TargetNoteRead {
        self.get(path)
            .cloned()
            .map(TargetNoteRead::Text)
            .unwrap_or(TargetNoteRead::Missing)
    }
}

#[cfg_attr(not(test), allow(dead_code))] // Independent classifier regressions.
pub(crate) struct CountingReader<R> {
    inner: R,
    pub reads: usize,
}

impl<R: TargetNoteReader> CountingReader<R> {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn new(inner: R) -> Self {
        Self { inner, reads: 0 }
    }
}

impl<R: TargetNoteReader> TargetNoteReader for CountingReader<R> {
    fn read_target(&mut self, path: &Path) -> TargetNoteRead {
        self.reads += 1;
        self.inner.read_target(path)
    }
}

//! Batch-level Pomodoro block tracking for `bob capture`.
//!
//! Every planner that selects, creates, moves, or reports a Pomodoro pushes
//! [`PomodoroBlockRef`]s onto its [`PlannedCaptureItem`](super::PlannedCaptureItem).
//! [`plan_capture_batch`](super::plan_capture_batch) owns a
//! [`PomodoroBlockTracker`] that resolves those refs against per-item
//! before/after day-file snapshots, auto-detects unreported touches, and
//! forwards block positions across items, so each touched Pomodoro is
//! reported once, in its final state, with the cumulative diff against the
//! ledger before the capture.
use super::*;

/// Informational role a capture played for one Pomodoro block, in
/// first-touch order. The app does not depend on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PomodoroBlockRole {
    Adjusted,
    Shifted,
    Started,
    Closed,
    Next,
    Linked,
    Unlinked,
    Changed,
}

/// Where a ref's headline sat before its item ran. Indices are 0-based
/// line numbers in that item's pre-state day text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PomodoroBlockBefore {
    At(usize),
    Resolve,
    Created,
}

/// One planner-reported touch of a Pomodoro block. `after` is the 0-based
/// headline index in that item's post-state text, or `None` to resolve it
/// through the item's line map. This type is never serialized;
/// [`PlannedCaptureItem`](super::PlannedCaptureItem) carries it only so the
/// batch loop can feed the tracker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PomodoroBlockRef {
    pub(super) role: PomodoroBlockRole,
    pub(super) before: PomodoroBlockBefore,
    pub(super) after: Option<usize>,
}

impl PomodoroBlockRef {
    pub(super) fn at(
        role: PomodoroBlockRole,
        before: usize,
        after: usize,
    ) -> Self {
        Self {
            role,
            before: PomodoroBlockBefore::At(before),
            after: Some(after),
        }
    }

    pub(super) fn created(role: PomodoroBlockRole, after: usize) -> Self {
        Self {
            role,
            before: PomodoroBlockBefore::Created,
            after: Some(after),
        }
    }

    pub(super) fn resolved(role: PomodoroBlockRole, after: usize) -> Self {
        Self {
            role,
            before: PomodoroBlockBefore::Resolve,
            after: Some(after),
        }
    }

    /// A moved-away link source: its headline is unchanged, so the batch
    /// loop resolves `after` through the item's line map.
    pub(super) fn unlinked_before(before: usize) -> Self {
        Self {
            role: PomodoroBlockRole::Unlinked,
            before: PomodoroBlockBefore::At(before),
            after: None,
        }
    }

    /// The started entry of a link/task start is also the appended Task
    /// Link's destination: both roles share one block.
    pub(super) fn started_and_linked(
        before: PomodoroBlockBefore,
        after: usize,
    ) -> [Self; 2] {
        [
            Self {
                role: PomodoroBlockRole::Started,
                before,
                after: Some(after),
            },
            Self {
                role: PomodoroBlockRole::Linked,
                before,
                after: Some(after),
            },
        ]
    }
}

/// A ref with both ends pinned to 0-based headline indices, plus the
/// release-mode fallback for a headline that changed without a ref.
struct ResolvedBlockRef {
    role: PomodoroBlockRole,
    before: PomodoroBlockBefore,
    after: usize,
    force_unchanged: bool,
}

/// One block tracked across the batch: its current headline index in the
/// latest post-state text, the roles in first-touch order, the block as it
/// looked before the batch, and whether the entry is new.
struct TrackedBlock {
    current: usize,
    roles: Vec<PomodoroBlockRole>,
    before_lines: Vec<String>,
    created: bool,
    force_unchanged: bool,
}

/// Batch-scoped tracker for one day file. Feed every item in order, then
/// call [`finish`](Self::finish) with the final staged day text.
pub(super) struct PomodoroBlockTracker {
    relative_target: String,
    tracked: Vec<TrackedBlock>,
}

impl PomodoroBlockTracker {
    pub(super) fn new(relative_target: String) -> Self {
        Self {
            relative_target,
            tracked: Vec::new(),
        }
    }

    /// Record one item's effect on the day file. `pre` is the text peeked
    /// before the item (or the file's `original` when the item loaded it
    /// first); `post` is the text after. Both `None` when the day file is
    /// still untouched, in which case there is nothing to track.
    pub(super) fn track_item(
        &mut self,
        pre: Option<&str>,
        post: Option<&str>,
        refs: Vec<PomodoroBlockRef>,
    ) {
        let (Some(pre), Some(post)) = (pre, post) else {
            debug_assert!(
                refs.is_empty(),
                "pomodoro block refs without a loaded day file"
            );
            return;
        };
        let pre_lines: Vec<&str> = pre.lines().collect();
        let post_lines: Vec<&str> = post.lines().collect();
        let pre_scan = capture_pomodoros::scan(pre);
        let post_scan = capture_pomodoros::scan(post);
        let pre_section_end =
            section_end(&pre_lines).unwrap_or(pre_lines.len());
        let post_section_end =
            section_end(&post_lines).unwrap_or(post_lines.len());
        let (old_to_new, new_to_old) = line_maps(&pre_lines, &post_lines);
        let mut resolved = resolve_refs(
            refs,
            &pre_scan,
            &post_scan,
            &pre_lines,
            &post_lines,
            &old_to_new,
            &new_to_old,
        );
        autodetect(
            &mut resolved,
            &pre_lines,
            &post_lines,
            &pre_scan,
            &post_scan,
            pre_section_end,
            post_section_end,
            &old_to_new,
            &new_to_old,
        );
        self.forward(
            &resolved,
            &old_to_new,
            &pre_lines,
            &post_lines,
            &post_scan,
        );
        self.merge_or_start(resolved, &pre_lines, pre_section_end);
    }

    /// Move every tracked headline into the latest post-state coordinates:
    /// refs that name the tracked headline win, otherwise the line map
    /// carries it forward. Myers reports a moved block as delete+insert,
    /// so an unmapped headline falls back to an exact-text search for a
    /// post entry. A headline found neither way means a planner changed
    /// it without a ref.
    fn forward(
        &mut self,
        resolved: &[ResolvedBlockRef],
        old_to_new: &BTreeMap<usize, usize>,
        pre_lines: &[&str],
        post_lines: &[&str],
        post_scan: &capture_pomodoros::PomodoroScan,
    ) {
        let mut drop_block = vec![false; self.tracked.len()];
        for (index, tracked) in self.tracked.iter_mut().enumerate() {
            let matches = resolved
                .iter()
                .filter(|item| {
                    item.before == PomodoroBlockBefore::At(tracked.current)
                })
                .collect::<Vec<_>>();
            if let Some(first) = matches.first() {
                debug_assert!(
                    matches.iter().all(|item| item.after == first.after),
                    "pomodoro block refs disagree on the new headline"
                );
                tracked.current = first.after;
            } else if let Some(mapped) = old_to_new.get(&tracked.current) {
                tracked.current = *mapped;
            } else if let Some(moved) = find_moved_headline(
                pre_lines.get(tracked.current).copied(),
                post_lines,
                post_scan,
                tracked.current,
            ) {
                tracked.current = moved;
            } else {
                debug_assert!(
                    false,
                    "tracked pomodoro headline lost without a ref"
                );
                drop_block[index] = true;
            }
        }
        let mut kept = Vec::with_capacity(self.tracked.len());
        for (index, tracked) in self.tracked.drain(..).enumerate() {
            if !drop_block[index] {
                kept.push(tracked);
            }
        }
        self.tracked = kept;
    }

    /// Fold resolved refs into tracked blocks by post-state headline, or
    /// start a new tracked block carrying the pre-image block bytes.
    fn merge_or_start(
        &mut self,
        resolved: Vec<ResolvedBlockRef>,
        pre_lines: &[&str],
        pre_section_end: usize,
    ) {
        for item in resolved {
            if let Some(tracked) = self
                .tracked
                .iter_mut()
                .find(|entry| entry.current == item.after)
            {
                if !tracked.roles.contains(&item.role) {
                    tracked.roles.push(item.role);
                }
                tracked.force_unchanged |= item.force_unchanged;
                continue;
            }
            let (before_lines, created) = match item.before {
                PomodoroBlockBefore::At(before) => (
                    capture_pomodoros::pomodoro_block_range(
                        pre_lines,
                        before,
                        pre_section_end,
                    )
                    .map(|index| pre_lines[index].to_string())
                    .collect(),
                    false,
                ),
                PomodoroBlockBefore::Created => (Vec::new(), true),
                // Resolution leaves only `At` and `Created` behind.
                PomodoroBlockBefore::Resolve => {
                    debug_assert!(
                        false,
                        "unresolved pomodoro block ref reached tracking"
                    );
                    continue;
                }
            };
            self.tracked.push(TrackedBlock {
                current: item.after,
                roles: vec![item.role],
                before_lines,
                created,
                force_unchanged: item.force_unchanged,
            });
        }
    }

    /// Re-extract every tracked block from the final staged day text and
    /// diff it against its pre-batch bytes.
    pub(super) fn finish(
        self,
        final_contents: Option<&str>,
    ) -> Vec<PomodoroBlockJson> {
        let Some(final_contents) = final_contents else {
            return Vec::new();
        };
        let final_lines: Vec<&str> = final_contents.lines().collect();
        let scan = capture_pomodoros::scan(final_contents);
        let entry_by_line = scan
            .entries
            .iter()
            .map(|entry| (entry.line.saturating_sub(1), entry))
            .collect::<BTreeMap<_, _>>();
        let end = section_end(&final_lines).unwrap_or(final_lines.len());
        let mut blocks = Vec::with_capacity(self.tracked.len());
        for tracked in self.tracked {
            let Some(entry) = entry_by_line.get(&tracked.current) else {
                debug_assert!(
                    false,
                    "tracked pomodoro headline is not an entry"
                );
                continue;
            };
            let range = capture_pomodoros::pomodoro_block_range(
                &final_lines,
                tracked.current,
                end,
            );
            let final_block = range
                .clone()
                .map(|index| final_lines[index].to_string())
                .collect::<Vec<_>>();
            let final_depths =
                block_depths(&final_lines, tracked.current, range);
            let before_refs = tracked
                .before_lines
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>();
            let before_depths =
                block_depths(&before_refs, 0, 0..tracked.before_lines.len());
            let status = match entry.state {
                capture_pomodoros::PomodoroState::Completed => {
                    PomodoroBlockStatus::Completed
                }
                capture_pomodoros::PomodoroState::Open
                    if entry.time_range.is_some() =>
                {
                    PomodoroBlockStatus::Running
                }
                capture_pomodoros::PomodoroState::Open => {
                    PomodoroBlockStatus::Queued
                }
            };
            let lines = if tracked.force_unchanged {
                final_block
                    .iter()
                    .zip(final_depths.iter().copied())
                    .map(|(text, depth)| PomodoroBlockLineJson {
                        text: text.clone(),
                        depth,
                        change: PomodoroBlockChange::Unchanged,
                        before: None,
                    })
                    .collect()
            } else {
                pair_lines(
                    &tracked.before_lines,
                    &before_depths,
                    &final_block,
                    &final_depths,
                    tracked.created,
                )
            };
            blocks.push(PomodoroBlockJson {
                relative_target: self.relative_target.clone(),
                line: tracked.current + 1,
                name: entry.name.clone(),
                time_range: entry.time_range.clone(),
                status,
                created: tracked.created,
                roles: tracked.roles,
                lines,
            });
        }
        blocks
    }
}

/// End of the `## Pomodoros` section in 0-based line coordinates.
fn section_end(lines: &[&str]) -> Option<usize> {
    pomodoro::pomodoros_section_range(lines).map(|range| range.end)
}

/// Exact-text fallback for a headline Myers left unmapped: a moved block
/// surfaces as delete+insert, so search the other snapshot's entry
/// headlines for the same bytes. Nearest to the old index wins; `None`
/// when no entry carries that text.
fn find_moved_headline(
    headline_text: Option<&str>,
    other_lines: &[&str],
    other_scan: &capture_pomodoros::PomodoroScan,
    hint: usize,
) -> Option<usize> {
    let wanted = headline_text?;
    other_scan
        .entries
        .iter()
        .map(|entry| entry.line.saturating_sub(1))
        .filter(|index| other_lines.get(*index).copied() == Some(wanted))
        .min_by_key(|index| index.abs_diff(hint))
}

/// Bidirectional line maps between two snapshots, built from the `Equal`
/// runs of a Myers line diff. A headline a ref does not describe as
/// changed stays byte-identical, so mapped lines resolve refs.
fn line_maps(
    pre: &[&str],
    post: &[&str],
) -> (BTreeMap<usize, usize>, BTreeMap<usize, usize>) {
    let mut old_to_new = BTreeMap::new();
    let mut new_to_old = BTreeMap::new();
    for op in similar::capture_diff_slices(similar::Algorithm::Myers, pre, post)
    {
        if let similar::DiffOp::Equal {
            old_index,
            new_index,
            len,
        } = op
        {
            for offset in 0..len {
                old_to_new.insert(old_index + offset, new_index + offset);
                new_to_old.insert(new_index + offset, old_index + offset);
            }
        }
    }
    (old_to_new, new_to_old)
}

/// Pin every planner ref to post-state coordinates. A `Resolve` before
/// reads the line map at `after`; a missing `after` reads it at `before`.
/// Every resolved `after` must be an entry headline in the post scan and
/// every `At` before one in the pre scan; anything else is a planner bug.
#[allow(clippy::too_many_arguments)]
fn resolve_refs(
    refs: Vec<PomodoroBlockRef>,
    pre_scan: &capture_pomodoros::PomodoroScan,
    post_scan: &capture_pomodoros::PomodoroScan,
    pre_lines: &[&str],
    post_lines: &[&str],
    old_to_new: &BTreeMap<usize, usize>,
    new_to_old: &BTreeMap<usize, usize>,
) -> Vec<ResolvedBlockRef> {
    let pre_entries = pre_scan
        .entries
        .iter()
        .map(|entry| entry.line.saturating_sub(1))
        .collect::<std::collections::BTreeSet<_>>();
    let post_entries = post_scan
        .entries
        .iter()
        .map(|entry| entry.line.saturating_sub(1))
        .collect::<std::collections::BTreeSet<_>>();
    let mut resolved = Vec::with_capacity(refs.len());
    for item in refs {
        match (item.before, item.after) {
            (PomodoroBlockBefore::At(before), Some(after)) => {
                if pre_entries.contains(&before)
                    && post_entries.contains(&after)
                {
                    resolved.push(ResolvedBlockRef {
                        role: item.role,
                        before: PomodoroBlockBefore::At(before),
                        after,
                        force_unchanged: false,
                    });
                } else {
                    debug_assert!(
                        false,
                        "pomodoro block ref names a non-entry headline: \
                         role {:?} before {before} (pre {pre_entries:?}) \
                         after {after} (post {post_entries:?})",
                        item.role,
                    );
                }
            }
            (PomodoroBlockBefore::At(before), None) => {
                if !pre_entries.contains(&before) {
                    debug_assert!(
                        false,
                        "pomodoro block ref names a non-entry headline"
                    );
                    continue;
                }
                match old_to_new.get(&before) {
                    Some(mapped) if post_entries.contains(mapped) => {
                        resolved.push(ResolvedBlockRef {
                            role: item.role,
                            before: PomodoroBlockBefore::At(before),
                            after: *mapped,
                            force_unchanged: false,
                        });
                    }
                    // The headline moved: locate it by its bytes.
                    _ => match find_moved_headline(
                        pre_lines.get(before).copied(),
                        post_lines,
                        post_scan,
                        before,
                    ) {
                        Some(moved) => resolved.push(ResolvedBlockRef {
                            role: item.role,
                            before: PomodoroBlockBefore::At(before),
                            after: moved,
                            force_unchanged: false,
                        }),
                        None => debug_assert!(
                            false,
                            "changed pomodoro headline needs an explicit after"
                        ),
                    },
                }
            }
            (PomodoroBlockBefore::Resolve, Some(after)) => {
                if !post_entries.contains(&after) {
                    debug_assert!(
                        false,
                        "pomodoro block ref names a non-entry headline"
                    );
                    continue;
                }
                match new_to_old.get(&after) {
                    Some(mapped) if pre_entries.contains(mapped) => {
                        resolved.push(ResolvedBlockRef {
                            role: item.role,
                            before: PomodoroBlockBefore::At(*mapped),
                            after,
                            force_unchanged: false,
                        });
                    }
                    // The headline moved: locate it by its bytes.
                    _ => match find_moved_headline(
                        post_lines.get(after).copied(),
                        pre_lines,
                        pre_scan,
                        after,
                    ) {
                        Some(moved) => resolved.push(ResolvedBlockRef {
                            role: item.role,
                            before: PomodoroBlockBefore::At(moved),
                            after,
                            force_unchanged: false,
                        }),
                        None => debug_assert!(
                            false,
                            "changed or created pomodoro needs an explicit before"
                        ),
                    },
                }
            }
            (PomodoroBlockBefore::Resolve, None)
            | (PomodoroBlockBefore::Created, None) => {
                debug_assert!(
                    false,
                    "pomodoro block ref needs an explicit after line"
                );
            }
            (PomodoroBlockBefore::Created, Some(after)) => {
                if post_entries.contains(&after) {
                    resolved.push(ResolvedBlockRef {
                        role: item.role,
                        before: PomodoroBlockBefore::Created,
                        after,
                        force_unchanged: false,
                    });
                } else {
                    debug_assert!(
                        false,
                        "pomodoro block ref names a non-entry headline"
                    );
                }
            }
        }
    }
    resolved
}

/// Report touches no planner claimed: a post entry whose block gained a
/// line or whose mapped pre-block lost one gets an implicit `changed`
/// ref. A rewritten headline with no ref is a planner bug: debug builds
/// panic, release builds show the block with every line unchanged rather
/// than a guessed diff. A pre-entry that vanished without a ref is the
/// same bug.
#[allow(clippy::too_many_arguments)]
fn autodetect(
    resolved: &mut Vec<ResolvedBlockRef>,
    pre_lines: &[&str],
    post_lines: &[&str],
    pre_scan: &capture_pomodoros::PomodoroScan,
    post_scan: &capture_pomodoros::PomodoroScan,
    pre_section_end: usize,
    post_section_end: usize,
    old_to_new: &BTreeMap<usize, usize>,
    new_to_old: &BTreeMap<usize, usize>,
) {
    let pre_entries = pre_scan
        .entries
        .iter()
        .map(|entry| entry.line.saturating_sub(1))
        .collect::<std::collections::BTreeSet<_>>();
    // Myers reports a moved block as delete+insert, so a line whose bytes
    // survive unmapped on the other side moved rather than changed.
    let pre_unmapped = pre_lines
        .iter()
        .enumerate()
        .filter(|(index, _)| !old_to_new.contains_key(index))
        .map(|(_, text)| *text)
        .collect::<std::collections::BTreeSet<_>>();
    let post_unmapped = post_lines
        .iter()
        .enumerate()
        .filter(|(index, _)| !new_to_old.contains_key(index))
        .map(|(_, text)| *text)
        .collect::<std::collections::BTreeSet<_>>();
    let mut claimed = resolved
        .iter()
        .map(|item| item.after)
        .collect::<std::collections::BTreeSet<_>>();
    for entry in &post_scan.entries {
        let headline = entry.line.saturating_sub(1);
        if claimed.contains(&headline) {
            continue;
        }
        let range = capture_pomodoros::pomodoro_block_range(
            post_lines,
            headline,
            post_section_end,
        );
        let gained = range.clone().any(|index| {
            !new_to_old.contains_key(&index)
                && !pre_unmapped.contains(post_lines[index])
        });
        let mapped_headline = new_to_old.get(&headline).copied();
        let lost = match mapped_headline {
            Some(mapped) if pre_entries.contains(&mapped) => {
                capture_pomodoros::pomodoro_block_range(
                    pre_lines,
                    mapped,
                    pre_section_end,
                )
                .any(|index| {
                    !old_to_new.contains_key(&index)
                        && !post_unmapped.contains(pre_lines[index])
                })
            }
            _ => false,
        };
        if !gained && !lost {
            continue;
        }
        match mapped_headline {
            Some(mapped) if pre_entries.contains(&mapped) => {
                claimed.insert(headline);
                resolved.push(ResolvedBlockRef {
                    role: PomodoroBlockRole::Changed,
                    before: PomodoroBlockBefore::At(mapped),
                    after: headline,
                    force_unchanged: false,
                });
            }
            _ => {
                // Genuinely new only when no line survived. Anything else
                // is a rewritten or moved headline with no ref: show the
                // block with every line unchanged rather than a guessed
                // diff. Link and task starts rewrite headlines without
                // refs until blocks_refs, so this is not a debug panic
                // (see the bead's follow-up notes).
                let any_mapped =
                    range.clone().any(|index| new_to_old.contains_key(&index));
                let all_new = range.clone().all(|index| {
                    new_to_old.contains_key(&index)
                        || !pre_unmapped.contains(post_lines[index])
                });
                claimed.insert(headline);
                if !any_mapped && all_new {
                    resolved.push(ResolvedBlockRef {
                        role: PomodoroBlockRole::Changed,
                        before: PomodoroBlockBefore::Created,
                        after: headline,
                        force_unchanged: false,
                    });
                } else {
                    resolved.push(ResolvedBlockRef {
                        role: PomodoroBlockRole::Changed,
                        before: PomodoroBlockBefore::Created,
                        after: headline,
                        force_unchanged: true,
                    });
                }
            }
        }
    }
    // A pre-entry with no mapping and no claimant (a placeholder a task
    // start consumed, for example) is dropped silently: link and task
    // starts legitimately reshape entries without refs until blocks_refs,
    // so this is not a debug panic (see the bead's follow-up notes).
}

/// Nesting level of every line in `range` relative to the headline: the
/// headline is 0, a list item counts its parent hops to the headline, a
/// non-list continuation line gets its parent list item's depth + 1, and
/// a blank line is 0. Mixed tabs and spaces work because parents come
/// from [`nearest_shallower_list_item_parent`].
fn block_depths(
    lines: &[&str],
    headline: usize,
    range: std::ops::Range<usize>,
) -> Vec<usize> {
    let joined = lines.join("\n");
    let spans = line_spans(&joined);
    let mut cache = BTreeMap::new();
    cache.insert(headline, 0);
    let mut depths = Vec::with_capacity(range.len());
    for index in range {
        if index == headline {
            depths.push(0);
            continue;
        }
        let text = lines.get(index).copied().unwrap_or("");
        if text.trim().is_empty() {
            depths.push(0);
            continue;
        }
        if list_item_body(text).is_some() {
            let mut depth = 0;
            let mut cursor = index;
            while let Some(parent) =
                nearest_shallower_list_item_parent(&spans, cursor)
            {
                depth += 1;
                if parent <= headline {
                    break;
                }
                cursor = parent;
            }
            cache.insert(index, depth);
            depths.push(depth);
        } else {
            let parent_depth =
                nearest_shallower_list_item_parent(&spans, index)
                    .map(|parent| {
                        if parent == headline {
                            0
                        } else {
                            cache.get(&parent).copied().unwrap_or(0)
                        }
                    })
                    .unwrap_or(0);
            let depth = parent_depth + 1;
            cache.insert(index, depth);
            depths.push(depth);
        }
    }
    depths
}

/// Pair pre-batch lines against final lines. Each `Replace` run becomes
/// `changed` rows (with `before`) for the first `min(old, new)` lines,
/// then `removed` rows for leftover old lines and `added` rows for
/// leftover new ones; adjacent `Delete` + `Insert` runs pair the same
/// way. A created (or otherwise empty-before) block is all `added`.
/// Removed lines keep their before-block depth.
fn pair_lines(
    before: &[String],
    before_depths: &[usize],
    final_block: &[String],
    final_depths: &[usize],
    created: bool,
) -> Vec<PomodoroBlockLineJson> {
    if created || before.is_empty() {
        return final_block
            .iter()
            .zip(final_depths.iter().copied())
            .map(|(text, depth)| PomodoroBlockLineJson {
                text: text.clone(),
                depth,
                change: PomodoroBlockChange::Added,
                before: None,
            })
            .collect();
    }
    let ops = similar::capture_diff_slices(
        similar::Algorithm::Myers,
        before,
        final_block,
    );
    let mut rows = Vec::new();
    let mut pending_old: Vec<(String, usize)> = Vec::new();
    let mut pending_new: Vec<(String, usize)> = Vec::new();
    let flush = |rows: &mut Vec<PomodoroBlockLineJson>,
                 pending_old: &mut Vec<(String, usize)>,
                 pending_new: &mut Vec<(String, usize)>| {
        let shared = pending_old.len().min(pending_new.len());
        for offset in 0..shared {
            let (old_text, _) = &pending_old[offset];
            let (new_text, new_depth) = &pending_new[offset];
            rows.push(PomodoroBlockLineJson {
                text: new_text.clone(),
                depth: *new_depth,
                change: PomodoroBlockChange::Changed,
                before: Some(old_text.clone()),
            });
        }
        for (old_text, old_depth) in pending_old.drain(..).skip(shared) {
            rows.push(PomodoroBlockLineJson {
                text: old_text,
                depth: old_depth,
                change: PomodoroBlockChange::Removed,
                before: None,
            });
        }
        for (new_text, new_depth) in pending_new.drain(..).skip(shared) {
            rows.push(PomodoroBlockLineJson {
                text: new_text,
                depth: new_depth,
                change: PomodoroBlockChange::Added,
                before: None,
            });
        }
    };
    for op in ops {
        match op {
            similar::DiffOp::Equal { new_index, len, .. } => {
                flush(&mut rows, &mut pending_old, &mut pending_new);
                for offset in 0..len {
                    rows.push(PomodoroBlockLineJson {
                        text: final_block[new_index + offset].clone(),
                        depth: final_depths[new_index + offset],
                        change: PomodoroBlockChange::Unchanged,
                        before: None,
                    });
                }
            }
            similar::DiffOp::Delete {
                old_index, old_len, ..
            } => {
                for offset in 0..old_len {
                    pending_old.push((
                        before[old_index + offset].clone(),
                        before_depths[old_index + offset],
                    ));
                }
            }
            similar::DiffOp::Insert {
                new_index, new_len, ..
            } => {
                for offset in 0..new_len {
                    pending_new.push((
                        final_block[new_index + offset].clone(),
                        final_depths[new_index + offset],
                    ));
                }
            }
            similar::DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                flush(&mut rows, &mut pending_old, &mut pending_new);
                let shared = old_len.min(new_len);
                for offset in 0..shared {
                    rows.push(PomodoroBlockLineJson {
                        text: final_block[new_index + offset].clone(),
                        depth: final_depths[new_index + offset],
                        change: PomodoroBlockChange::Changed,
                        before: Some(before[old_index + offset].clone()),
                    });
                }
                for offset in shared..old_len {
                    rows.push(PomodoroBlockLineJson {
                        text: before[old_index + offset].clone(),
                        depth: before_depths[old_index + offset],
                        change: PomodoroBlockChange::Removed,
                        before: None,
                    });
                }
                for offset in shared..new_len {
                    rows.push(PomodoroBlockLineJson {
                        text: final_block[new_index + offset].clone(),
                        depth: final_depths[new_index + offset],
                        change: PomodoroBlockChange::Added,
                        before: None,
                    });
                }
            }
        }
    }
    flush(&mut rows, &mut pending_old, &mut pending_new);
    rows
}

/// One Pomodoro block in its final state, at batch level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroBlockJson {
    pub(super) relative_target: String,
    pub(super) line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) time_range: Option<String>,
    pub(super) status: PomodoroBlockStatus,
    pub(super) created: bool,
    pub(super) roles: Vec<PomodoroBlockRole>,
    pub(super) lines: Vec<PomodoroBlockLineJson>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PomodoroBlockStatus {
    Completed,
    Running,
    Queued,
}

/// One verbatim line of a block: `text` never carries a terminator, and
/// `before` holds the old text only on `changed` rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroBlockLineJson {
    pub(super) text: String,
    pub(super) depth: usize,
    pub(super) change: PomodoroBlockChange,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) before: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PomodoroBlockChange {
    Unchanged,
    Added,
    Removed,
    Changed,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines_of(text: &str) -> Vec<&str> {
        text.lines().collect()
    }

    #[test]
    fn block_range_keeps_tab_children_and_interior_blanks() {
        let text = "## Pomodoros\n\
             - [ ] (**0620-0710** [t:: 50m]) — CLEANUP\n\
             \t- [[sase#^one]]\n\
             \n\
             \t- [[sase#^two]]\n\
             - [ ] () — GTD\n";
        let lines = lines_of(text);
        assert_eq!(
            capture_pomodoros::pomodoro_block_range(&lines, 1, lines.len()),
            1..5
        );
    }

    #[test]
    fn block_range_trims_trailing_blanks_and_stops_at_zero_indent() {
        let text = "## Pomodoros\n\
             - [ ] () — A\n\
             \t- child\n\
             \n\
             \n\
             ## Later\n";
        let lines = lines_of(text);
        assert_eq!(
            capture_pomodoros::pomodoro_block_range(&lines, 1, lines.len()),
            1..3
        );
    }

    #[test]
    fn block_range_supports_two_space_and_mixed_indentation() {
        let text = concat!(
            "## Pomodoros\n",
            "- [ ] () — A\n",
            "  - two spaces\n",
            "\t- tab\n",
            "\t  - mixed\n",
            "- [ ] () — B\n",
        );
        let lines = lines_of(text);
        assert_eq!(
            capture_pomodoros::pomodoro_block_range(&lines, 1, lines.len()),
            1..5
        );
    }

    #[test]
    fn block_range_keeps_fenced_child_lines_verbatim() {
        let text = "## Pomodoros\n\
             - [ ] () — A\n\
             \t```\n\
             \tcode\n\
             \t```\n\
             \t- after fence\n\
             - [ ] () — B\n";
        let lines = lines_of(text);
        assert_eq!(
            capture_pomodoros::pomodoro_block_range(&lines, 1, lines.len()),
            1..6
        );
    }

    #[test]
    fn block_range_stops_at_section_end_and_covers_last_entry() {
        let text = "## Pomodoros\n\
             - [ ] () — A\n\
             - [ ] () — B\n\
             \t- child\n";
        let lines = lines_of(text);
        assert_eq!(capture_pomodoros::pomodoro_block_range(&lines, 1, 3), 1..2);
        assert_eq!(
            capture_pomodoros::pomodoro_block_range(&lines, 2, lines.len()),
            2..4
        );
    }

    #[test]
    fn depth_counts_nested_list_items() {
        let text = "## Pomodoros\n\
             - [ ] () — A\n\
             \t- one\n\
             \t\t- two\n\
             \t\t\t- three\n";
        let lines = lines_of(text);
        assert_eq!(block_depths(&lines, 1, 1..5), vec![0, 1, 2, 3]);
    }

    #[test]
    fn depth_supports_two_space_and_mixed_indentation() {
        let text = concat!(
            "## Pomodoros\n",
            "- [ ] () — A\n",
            "  - two spaces\n",
            "\t- tab\n",
        );
        let lines = lines_of(text);
        assert_eq!(block_depths(&lines, 1, 1..4), vec![0, 1, 1]);
    }

    #[test]
    fn depth_continuation_lines_take_parent_depth_plus_one() {
        let text = "## Pomodoros\n\
             - [ ] () — A\n\
             \t- one\n\
             \t  wrapped text\n\
             \tplain continuation\n\
             \n";
        let lines = lines_of(text);
        assert_eq!(block_depths(&lines, 1, 1..5), vec![0, 1, 2, 1]);
    }

    #[test]
    fn pairing_replace_becomes_changed_with_before() {
        let before = vec!["- [ ] (**0620-0710** [t:: 50m]) — X".to_string()];
        let after = vec!["- [ ] (**0620-0735** [t:: 75m]) — X".to_string()];
        let rows = pair_lines(&before, &[0], &after, &[0], false);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].change, PomodoroBlockChange::Changed);
        assert_eq!(rows[0].before.as_deref(), Some(before[0].as_str()));
        assert_eq!(rows[0].depth, 0);
    }

    #[test]
    fn pairing_pure_insert_is_added_and_pure_delete_is_removed() {
        let before = vec!["- [ ] () — A".to_string()];
        let after =
            vec!["- [ ] () — A".to_string(), "\t- new child".to_string()];
        let rows = pair_lines(&before, &[0], &after, &[0, 1], false);
        assert_eq!(
            rows.iter().map(|row| row.change).collect::<Vec<_>>(),
            vec![PomodoroBlockChange::Unchanged, PomodoroBlockChange::Added]
        );
        let rows = pair_lines(&after, &[0, 1], &before, &[0], false);
        assert_eq!(
            rows.iter().map(|row| row.change).collect::<Vec<_>>(),
            vec![PomodoroBlockChange::Unchanged, PomodoroBlockChange::Removed]
        );
        assert_eq!(rows[1].depth, 1);
    }

    #[test]
    fn pairing_uneven_replace_splits_changed_added_removed() {
        let before = vec!["- old".to_string()];
        let after = vec!["- new".to_string(), "\t- extra".to_string()];
        let rows = pair_lines(&before, &[0], &after, &[0, 1], false);
        assert_eq!(
            rows.iter().map(|row| row.change).collect::<Vec<_>>(),
            vec![PomodoroBlockChange::Changed, PomodoroBlockChange::Added]
        );
        assert_eq!(rows[0].before.as_deref(), Some("- old"));

        let rows = pair_lines(&after, &[0, 1], &before, &[0], false);
        assert_eq!(
            rows.iter().map(|row| row.change).collect::<Vec<_>>(),
            vec![PomodoroBlockChange::Changed, PomodoroBlockChange::Removed]
        );
        assert!(rows[0].before.is_some());
    }

    #[test]
    fn pairing_created_block_is_all_added() {
        let after = vec!["- [ ] () — NEW".to_string(), "\t- child".to_string()];
        let rows = pair_lines(&[], &[], &after, &[0, 1], true);
        assert!(rows
            .iter()
            .all(|row| row.change == PomodoroBlockChange::Added));
        assert!(rows.iter().all(|row| row.before.is_none()));
    }

    fn tracker_with_items(
        items: &[(&str, &str, Vec<PomodoroBlockRef>)],
        final_contents: &str,
        relative_target: &str,
    ) -> Vec<PomodoroBlockJson> {
        let mut tracker =
            PomodoroBlockTracker::new(relative_target.to_string());
        for (pre, post, refs) in items {
            tracker.track_item(Some(pre), Some(post), refs.clone());
        }
        tracker.finish(Some(final_contents))
    }

    const LEDGER: &str = "## Pomodoros\n\
         - [ ] (**0620-0710** [t:: 50m]) — CLEANUP\n\
         \t- [[sase#^one]]\n\
         - [ ] () — GTD\n\
         \t- [[#^gtd]]\n";

    #[test]
    fn tracker_reports_one_cumulative_block_across_items() {
        let adjusted = LEDGER
            .replace("(**0620-0710** [t:: 50m])", "(**0620-0720** [t:: 60m])");
        let closed =
            adjusted.replace("- [ ] (**0620-0720**", "- [x] (**0620-0720**");
        let blocks = tracker_with_items(
            &[
                (
                    LEDGER,
                    adjusted.as_str(),
                    vec![PomodoroBlockRef::at(
                        PomodoroBlockRole::Adjusted,
                        1,
                        1,
                    )],
                ),
                (
                    adjusted.as_str(),
                    closed.as_str(),
                    vec![PomodoroBlockRef::at(PomodoroBlockRole::Closed, 1, 1)],
                ),
            ],
            closed.as_str(),
            "2026/20260930.md",
        );
        assert_eq!(blocks.len(), 1);
        let block = &blocks[0];
        assert_eq!(block.line, 2);
        assert_eq!(block.name.as_deref(), Some("CLEANUP"));
        assert_eq!(block.time_range.as_deref(), Some("0620-0720"));
        assert_eq!(block.status, PomodoroBlockStatus::Completed);
        assert!(!block.created);
        assert_eq!(
            block.roles,
            vec![PomodoroBlockRole::Adjusted, PomodoroBlockRole::Closed]
        );
        assert_eq!(block.lines[0].change, PomodoroBlockChange::Changed);
        assert_eq!(
            block.lines[0].before.as_deref(),
            Some("- [ ] (**0620-0710** [t:: 50m]) — CLEANUP")
        );
        assert_eq!(block.lines[1].change, PomodoroBlockChange::Unchanged);
    }

    #[test]
    fn tracker_autodetects_an_unreported_child_insert() {
        let text = "## Pomodoros\n- [ ] () — A\n";
        let with_child = "## Pomodoros\n- [ ] () — A\n\t- new\n";
        let blocks = tracker_with_items(
            &[(text, with_child, vec![])],
            with_child,
            "d.md",
        );
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].roles, vec![PomodoroBlockRole::Changed]);
        assert_eq!(blocks[0].lines[1].change, PomodoroBlockChange::Added);
    }

    #[test]
    fn tracker_forwards_a_tracked_block_across_an_insert_above() {
        let pre = "## Pomodoros\n- [ ] () — A\n\t- child\n";
        let adjusted =
            "## Pomodoros\n- [ ] (**0720-0745** [t:: 25m]) — A\n\t- child\n";
        let with_new = concat!(
            "## Pomodoros\n",
            "- [ ] () — NEW\n",
            "- [ ] (**0720-0745** [t:: 25m]) — A\n",
            "\t- child\n",
        );
        let blocks = tracker_with_items(
            &[
                (
                    pre,
                    adjusted,
                    vec![PomodoroBlockRef::at(
                        PomodoroBlockRole::Adjusted,
                        1,
                        1,
                    )],
                ),
                (
                    adjusted,
                    with_new,
                    vec![PomodoroBlockRef::created(PomodoroBlockRole::Next, 1)],
                ),
            ],
            with_new,
            "d.md",
        );
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].name.as_deref(), Some("A"));
        assert_eq!(blocks[0].line, 3);
        assert_eq!(blocks[0].roles, vec![PomodoroBlockRole::Adjusted]);
        assert_eq!(blocks[1].name.as_deref(), Some("NEW"));
        assert_eq!(blocks[1].line, 2);
        assert!(blocks[1].created);
    }

    #[test]
    fn tracker_resolves_created_before_and_reports_roles_once() {
        let text = "## Pomodoros\n- [ ] () — A\n";
        let with_child = "## Pomodoros\n- [ ] () — A\n\t- new\n";
        let blocks = tracker_with_items(
            &[(
                text,
                with_child,
                vec![PomodoroBlockRef::resolved(PomodoroBlockRole::Next, 1)],
            )],
            with_child,
            "d.md",
        );
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].roles, vec![PomodoroBlockRole::Next]);
        assert!(!blocks[0].created);
    }

    #[test]
    #[cfg_attr(not(debug_assertions), ignore)]
    #[should_panic(expected = "non-entry headline")]
    fn resolution_rejects_a_non_entry_after() {
        let mut tracker = PomodoroBlockTracker::new("d.md".to_string());
        tracker.track_item(
            Some(LEDGER),
            Some(LEDGER),
            vec![PomodoroBlockRef::at(PomodoroBlockRole::Adjusted, 1, 0)],
        );
    }

    #[test]
    fn unreported_headline_rewrite_emits_unchanged_block() {
        // Link and task starts rewrite headlines without refs until
        // blocks_refs: never a guessed diff, and never a panic.
        let rewritten = LEDGER.replace(
            "- [ ] (**0620-0710** [t:: 50m]) — CLEANUP",
            "- [ ] (**0620-0735** [t:: 75m]) — CLEANUP",
        );
        let blocks = tracker_with_items(
            &[(LEDGER, &rewritten, vec![])],
            &rewritten,
            "d.md",
        );
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].roles, vec![PomodoroBlockRole::Changed]);
        assert!(blocks[0]
            .lines
            .iter()
            .all(|row| row.change == PomodoroBlockChange::Unchanged));
    }

    #[test]
    fn vanished_entry_is_dropped_silently() {
        // Task starts consume placeholders without refs until blocks_refs.
        let dropped = "## Pomodoros\n- [ ] () — GTD\n\t- [[#^gtd]]\n";
        let blocks =
            tracker_with_items(&[(LEDGER, dropped, vec![])], dropped, "d.md");
        assert!(blocks.is_empty());
    }

    #[test]
    #[cfg_attr(not(debug_assertions), ignore)]
    #[should_panic(expected = "tracked pomodoro headline lost")]
    fn forwarding_panics_when_a_tracked_headline_is_deleted() {
        let mut tracker = PomodoroBlockTracker::new("d.md".to_string());
        tracker.track_item(
            Some(LEDGER),
            Some(LEDGER),
            vec![PomodoroBlockRef::at(PomodoroBlockRole::Adjusted, 1, 1)],
        );
        let dropped = "## Pomodoros\n- [ ] () — GTD\n\t- [[#^gtd]]\n";
        tracker.track_item(Some(LEDGER), Some(dropped), vec![]);
    }
}

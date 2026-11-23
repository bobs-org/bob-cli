//! Shared block-diff helpers for batch-level block previews.
//!
//! Both [`PomodoroBlockTracker`](super::PomodoroBlockTracker) and
//! [`TaskBlockTracker`](super::TaskBlockTracker) diff a before-block
//! against its final block with the same line schema, depths, and Myers
//! pairing, so this module owns those pieces. Serialized JSON stays
//! identical for existing consumers.
use super::*;

/// One verbatim line of a block: `text` never carries a terminator, and
/// `before` holds the old text only on `changed` rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct BlockLineJson {
    pub(super) text: String,
    pub(super) depth: usize,
    pub(super) change: BlockLineChange,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) before: Option<String>,
    /// Successor-link marker (`docs/task-dependencies.md` §12.5):
    /// `"unblocked"` on added lines that equal a surviving successor
    /// bullet. Always `None` on task blocks and on non-successor lines,
    /// so unrelated output stays byte-identical.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) reason: Option<&'static str>,
    /// Merge provenance: `"<relative_path>#^<block_id>"` matching
    /// `merges[].source` on rows a merge in this capture landed.
    /// Omitted everywhere else. Additive and byte-identical when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) origin: Option<String>,
    /// Requirement annotation from the recognizer: `depth` (1 is a
    /// direct child of the owner) and `state`
    /// ([`EffectiveReqState::as_str`](crate::native::task_reqs::EffectiveReqState::as_str)).
    /// Present only on requirement head lines, so unrelated output stays
    /// byte-identical. Clients draw the diamond from this object and
    /// never infer it from `[ ]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) req: Option<crate::native::task_reqs::ReqLineJson>,
}

impl BlockLineJson {
    pub(super) fn new(
        text: String,
        depth: usize,
        change: BlockLineChange,
        before: Option<String>,
    ) -> Self {
        Self {
            text,
            depth,
            change,
            before,
            reason: None,
            origin: None,
            req: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum BlockLineChange {
    Unchanged,
    Added,
    Removed,
    Changed,
}

/// Bidirectional line maps between two snapshots, built from the `Equal`
/// runs of a Myers line diff. A headline a ref does not describe as
/// changed stays byte-identical, so mapped lines resolve refs.
pub(super) fn line_maps(
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

/// Nesting level of every line in `range` relative to the headline: the
/// headline is 0, a list item counts its parent hops to the headline, a
/// non-list continuation line gets its parent list item's depth + 1, and
/// a blank line is 0. Mixed tabs and spaces work because parents come
/// from [`nearest_shallower_list_item_parent`].
pub(crate) fn block_depths(
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
pub(super) fn pair_lines(
    before: &[String],
    before_depths: &[usize],
    final_block: &[String],
    final_depths: &[usize],
    created: bool,
) -> Vec<BlockLineJson> {
    if created || before.is_empty() {
        return final_block
            .iter()
            .zip(final_depths.iter().copied())
            .map(|(text, depth)| {
                BlockLineJson::new(
                    text.clone(),
                    depth,
                    BlockLineChange::Added,
                    None,
                )
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
    let flush = |rows: &mut Vec<BlockLineJson>,
                 pending_old: &mut Vec<(String, usize)>,
                 pending_new: &mut Vec<(String, usize)>| {
        let shared = pending_old.len().min(pending_new.len());
        for offset in 0..shared {
            let (old_text, _) = &pending_old[offset];
            let (new_text, new_depth) = &pending_new[offset];
            rows.push(BlockLineJson::new(
                new_text.clone(),
                *new_depth,
                BlockLineChange::Changed,
                Some(old_text.clone()),
            ));
        }
        for (old_text, old_depth) in pending_old.drain(..).skip(shared) {
            rows.push(BlockLineJson::new(
                old_text,
                old_depth,
                BlockLineChange::Removed,
                None,
            ));
        }
        for (new_text, new_depth) in pending_new.drain(..).skip(shared) {
            rows.push(BlockLineJson::new(
                new_text,
                new_depth,
                BlockLineChange::Added,
                None,
            ));
        }
    };
    for op in ops {
        match op {
            similar::DiffOp::Equal { new_index, len, .. } => {
                flush(&mut rows, &mut pending_old, &mut pending_new);
                for offset in 0..len {
                    rows.push(BlockLineJson::new(
                        final_block[new_index + offset].clone(),
                        final_depths[new_index + offset],
                        BlockLineChange::Unchanged,
                        None,
                    ));
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
                    rows.push(BlockLineJson::new(
                        final_block[new_index + offset].clone(),
                        final_depths[new_index + offset],
                        BlockLineChange::Changed,
                        Some(before[old_index + offset].clone()),
                    ));
                }
                for offset in shared..old_len {
                    rows.push(BlockLineJson::new(
                        before[old_index + offset].clone(),
                        before_depths[old_index + offset],
                        BlockLineChange::Removed,
                        None,
                    ));
                }
                for offset in shared..new_len {
                    rows.push(BlockLineJson::new(
                        final_block[new_index + offset].clone(),
                        final_depths[new_index + offset],
                        BlockLineChange::Added,
                        None,
                    ));
                }
            }
        }
    }
    flush(&mut rows, &mut pending_old, &mut pending_new);
    rows
}

/// Snapshot the recognizer uses for one side of a block diff: the whole
/// note text plus the 0-based start of this block inside it.
pub(super) struct ReqSnapshot<'a> {
    pub(super) contents: &'a str,
    pub(super) start: usize,
    pub(super) settings: &'a note_tasks::NoteTaskSettings,
}

/// Fill `req` from the recognizer. Unchanged, added, and changed rows
/// use the after snapshot; removed rows use the before snapshot. Owner
/// done → met and owner cancelled → moot follow from those snapshots.
pub(super) fn annotate_req_lines(
    lines: &mut [BlockLineJson],
    before: Option<ReqSnapshot<'_>>,
    after: Option<ReqSnapshot<'_>>,
) {
    let before_map = before.as_ref().map(|snapshot| {
        crate::native::task_reqs::req_line_map(
            snapshot.contents,
            snapshot.settings,
        )
    });
    let after_map = after.as_ref().map(|snapshot| {
        crate::native::task_reqs::req_line_map(
            snapshot.contents,
            snapshot.settings,
        )
    });
    let before_start =
        before.as_ref().map(|snapshot| snapshot.start).unwrap_or(0);
    let after_start =
        after.as_ref().map(|snapshot| snapshot.start).unwrap_or(0);
    let mut before_index = 0usize;
    let mut after_index = 0usize;
    for row in lines {
        match row.change {
            BlockLineChange::Removed => {
                if let Some(map) = before_map.as_ref() {
                    row.req = map.get(&(before_start + before_index)).cloned();
                }
                before_index += 1;
            }
            BlockLineChange::Added => {
                if let Some(map) = after_map.as_ref() {
                    row.req = map.get(&(after_start + after_index)).cloned();
                }
                after_index += 1;
            }
            BlockLineChange::Unchanged | BlockLineChange::Changed => {
                if let Some(map) = after_map.as_ref() {
                    row.req = map.get(&(after_start + after_index)).cloned();
                }
                before_index += 1;
                after_index += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::task_reqs::ReqLineJson;
    use std::path::Path;

    fn settings() -> note_tasks::NoteTaskSettings {
        note_tasks::read_settings(Path::new("/no-bob-vault"))
    }

    fn req(depth: usize, state: &'static str) -> ReqLineJson {
        ReqLineJson { depth, state }
    }

    #[test]
    fn annotate_req_lines_uses_after_snapshot_except_removed() {
        let settings = settings();
        let before = concat!(
            "- [ ] #task Owner ^ex\n",
            "\t- [ ] Keep\n",
            "\t- [ ] Nested parent\n",
            "\t\t- [ ] Nested child\n",
            "\t- [ ] Dropped later\n",
        );
        let after = concat!(
            "- [x] #task Owner [completion:: 2026-06-15] ^ex\n",
            "\t- [ ] Keep\n",
            "\t- [ ] Nested parent\n",
            "\t\t- [ ] Nested child\n",
            "\t- [ ] Added\n",
        );
        let mut lines = vec![
            BlockLineJson::new(
                after.lines().next().unwrap().to_string(),
                0,
                BlockLineChange::Changed,
                Some(before.lines().next().unwrap().to_string()),
            ),
            BlockLineJson::new(
                "\t- [ ] Keep".into(),
                1,
                BlockLineChange::Unchanged,
                None,
            ),
            BlockLineJson::new(
                "\t- [ ] Nested parent".into(),
                1,
                BlockLineChange::Unchanged,
                None,
            ),
            BlockLineJson::new(
                "\t\t- [ ] Nested child".into(),
                2,
                BlockLineChange::Unchanged,
                None,
            ),
            BlockLineJson::new(
                "\t- [ ] Dropped later".into(),
                1,
                BlockLineChange::Removed,
                None,
            ),
            BlockLineJson::new(
                "\t- [ ] Added".into(),
                1,
                BlockLineChange::Added,
                None,
            ),
        ];
        annotate_req_lines(
            &mut lines,
            Some(ReqSnapshot {
                contents: before,
                start: 0,
                settings: &settings,
            }),
            Some(ReqSnapshot {
                contents: after,
                start: 0,
                settings: &settings,
            }),
        );
        assert_eq!(
            lines
                .iter()
                .map(|row| (row.change, row.req.clone()))
                .collect::<Vec<_>>(),
            vec![
                (BlockLineChange::Changed, None),
                (BlockLineChange::Unchanged, Some(req(1, "met"))),
                (BlockLineChange::Unchanged, Some(req(1, "met"))),
                (BlockLineChange::Unchanged, Some(req(2, "met"))),
                (BlockLineChange::Removed, Some(req(1, "pending"))),
                (BlockLineChange::Added, Some(req(1, "met"))),
            ]
        );
    }

    #[test]
    fn annotate_req_lines_omits_notes_and_drawers() {
        let settings = settings();
        let after = concat!(
            "- [ ] #task Owner ^ex\n",
            "\t- [ ] A req\n",
            "\t\t- attached note\n",
            "\t- NOTES\n",
            "\t\t- drawer note\n",
        );
        let after_block: Vec<String> =
            after.lines().map(str::to_owned).collect();
        let after_depths = vec![0, 1, 2, 1, 2];
        let mut lines = pair_lines(&[], &[], &after_block, &after_depths, true);
        annotate_req_lines(
            &mut lines,
            None,
            Some(ReqSnapshot {
                contents: after,
                start: 0,
                settings: &settings,
            }),
        );
        assert_eq!(
            lines.iter().map(|row| row.req.clone()).collect::<Vec<_>>(),
            vec![None, Some(req(1, "pending")), None, None, None]
        );
    }

    #[test]
    fn annotate_req_lines_reads_dropped_carried_excluded_and_moot() {
        let settings = settings();
        let after = concat!(
            "- [-] #task Owner ^ex\n",
            "\t- [ ] open under cancelled\n",
            "\t- [-] dropped req\n",
            "\t\t- [ ] excluded child\n",
            "\t- [>] [[#^own|carried req]]\n",
        );
        let after_block: Vec<String> =
            after.lines().map(str::to_owned).collect();
        let after_depths = vec![0, 1, 1, 2, 1];
        let mut lines = pair_lines(&[], &[], &after_block, &after_depths, true);
        annotate_req_lines(
            &mut lines,
            None,
            Some(ReqSnapshot {
                contents: after,
                start: 0,
                settings: &settings,
            }),
        );
        assert_eq!(
            lines.iter().map(|row| row.req.clone()).collect::<Vec<_>>(),
            vec![
                None,
                Some(req(1, "moot")),
                Some(req(1, "dropped")),
                Some(req(2, "excluded")),
                Some(req(1, "carried")),
            ]
        );
    }
}

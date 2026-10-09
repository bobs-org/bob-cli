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
            .map(|(text, depth)| BlockLineJson {
                text: text.clone(),
                depth,
                change: BlockLineChange::Added,
                before: None,
                reason: None,
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
            rows.push(BlockLineJson {
                text: new_text.clone(),
                depth: *new_depth,
                change: BlockLineChange::Changed,
                before: Some(old_text.clone()),
                reason: None,
            });
        }
        for (old_text, old_depth) in pending_old.drain(..).skip(shared) {
            rows.push(BlockLineJson {
                text: old_text,
                depth: old_depth,
                change: BlockLineChange::Removed,
                before: None,
                reason: None,
            });
        }
        for (new_text, new_depth) in pending_new.drain(..).skip(shared) {
            rows.push(BlockLineJson {
                text: new_text,
                depth: new_depth,
                change: BlockLineChange::Added,
                before: None,
                reason: None,
            });
        }
    };
    for op in ops {
        match op {
            similar::DiffOp::Equal { new_index, len, .. } => {
                flush(&mut rows, &mut pending_old, &mut pending_new);
                for offset in 0..len {
                    rows.push(BlockLineJson {
                        text: final_block[new_index + offset].clone(),
                        depth: final_depths[new_index + offset],
                        change: BlockLineChange::Unchanged,
                        before: None,
                        reason: None,
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
                    rows.push(BlockLineJson {
                        text: final_block[new_index + offset].clone(),
                        depth: final_depths[new_index + offset],
                        change: BlockLineChange::Changed,
                        before: Some(before[old_index + offset].clone()),
                        reason: None,
                    });
                }
                for offset in shared..old_len {
                    rows.push(BlockLineJson {
                        text: before[old_index + offset].clone(),
                        depth: before_depths[old_index + offset],
                        change: BlockLineChange::Removed,
                        before: None,
                        reason: None,
                    });
                }
                for offset in shared..new_len {
                    rows.push(BlockLineJson {
                        text: final_block[new_index + offset].clone(),
                        depth: final_depths[new_index + offset],
                        change: BlockLineChange::Added,
                        before: None,
                        reason: None,
                    });
                }
            }
        }
    }
    flush(&mut rows, &mut pending_old, &mut pending_new);
    rows
}

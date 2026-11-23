//! Pre-close requirement groups and snapshot-bound row identity.
//!
//! Pure row, group, and copy model for the capture successor card
//! (`docs/task-requirements.md` §7). Managed Done collectors attach one
//! [`ReqCloseOwner`] per successful open-to-Done transition, bound to the
//! exact pre-close owner-block bytes, before that text is discarded.
//! This phase does not write requirement state or advertise a public
//! req capability.

use std::path::Path;

use sha2::{Digest, Sha256};

use super::super::{
    capture::line_spans,
    note_tasks::{self, stable_task_key, NoteTask, NoteTaskSettings},
    task_reqs::{
        self, req_glyph, req_path, req_row_key, EffectiveReqState, ReqNode,
        MAX_REQ_DEPTH,
    },
};
use super::is_recurring_task_line;

/// Canonical copy frozen by the RV vectors.
pub(crate) mod copy {

    /// Group caption under the title.
    pub(crate) const WHY: &str = "Met when it closes, unless you say otherwise";
    /// Inline drop-reason prompt.
    #[cfg(test)]
    pub(crate) const DROP_PROMPT: &str = "Why drop it? (optional)";
    /// Per-row footer of the requirement group.
    pub(crate) const FOOTER: &str =
        "⌃X met · ⌃⇧X drop · ⌃⇧T own task · space pull in · ⌃J jump";
    /// Jump-pin hint on a requirement row.
    #[cfg(test)]
    pub(crate) const JUMP_HINT: &str =
        "↗ jumps to the requirement — ⌃⇧T makes it a task";
    /// Human CLI hint under a requirement group.
    pub(crate) const HINT: &str =
        "-k N own task · -X N[=why] drop · -p N pull in";
    /// Teaching line shared with the contract page.
    #[cfg(test)]
    pub(crate) const TEACHING: &str =
        "Met when this task closes. Drop it or make it a task if the scope changes.";

    pub(crate) fn quoted(text: &str) -> String {
        format!("“{text}”")
    }

    pub(crate) fn group_title(owner_text: &str) -> String {
        format!("Requirements · {}", quoted(owner_text))
    }

    /// `1 requirement`, `{n} requirements`, or
    /// `{n} requirements in {b} branches`.
    pub(crate) fn scope(pending: usize, branches: usize) -> String {
        if branches <= 1 {
            if pending == 1 {
                "1 requirement".to_string()
            } else {
                format!("{pending} requirements")
            }
        } else {
            format!("{pending} requirements in {branches} branches")
        }
    }

    /// Historical fold line. Omits zero segments; `None` when every
    /// count is zero. Counts explicit historical states only.
    pub(crate) fn history(
        dropped: usize,
        carried: usize,
        met: usize,
    ) -> Option<String> {
        let mut parts = Vec::new();
        if dropped > 0 {
            parts.push(format!("{dropped} dropped earlier"));
        }
        if carried > 0 {
            parts.push(format!("{carried} carried earlier"));
        }
        if met > 0 {
            parts.push(format!("{met} met earlier"));
        }
        (!parts.is_empty()).then(|| format!("· {}", parts.join(" · ")))
    }

    pub(crate) fn overlap(ancestor: &str) -> String {
        format!(
            "{} already takes this branch — restage it as met to choose here",
            quoted(ancestor)
        )
    }

    #[cfg(test)]
    pub(crate) fn nested_go_with_it(count: usize) -> String {
        format!("· {count} nested go with it")
    }

    pub(crate) fn met_receipt(count: usize, owner_text: &str) -> String {
        format!("✓ {count} requirements met with {}", quoted(owner_text))
    }

    pub(crate) fn human_met_line(count: usize, owner_text: &str) -> String {
        met_receipt(count, owner_text)
    }

    pub(crate) fn drop_receipt(text: &str, reason: Option<&str>) -> String {
        let mut line = format!("⊘ Dropped {}", quoted(text));
        if let Some(reason) = reason.filter(|reason| !reason.is_empty()) {
            line.push_str(" · ");
            line.push_str(&quoted(reason));
        }
        line
    }

    pub(crate) fn carry_receipt(text: &str, pulled: bool) -> String {
        let mut line = format!("→ {} is its own task now", quoted(text));
        if pulled {
            line.push_str(" · pulled in");
        }
        line
    }

    #[cfg(test)]
    pub(crate) fn summary(
        met: usize,
        dropped: usize,
        carried: usize,
    ) -> String {
        format!("reqs ✓{met} ⊘{dropped} →{carried}")
    }

    pub(crate) fn accessibility(depth: usize, verb: &str) -> String {
        format!("requirement, level {depth}, {verb}")
    }

    pub(crate) fn would_met(count: usize, owner_text: &str) -> String {
        format!(
            "would meet {count} requirements with {}",
            quoted(owner_text)
        )
    }
}

/// Bob-authored consequence copy for one requirement row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReqConsequences {
    pub(crate) met: String,
    pub(crate) drop: String,
    pub(crate) carry: String,
    pub(crate) carry_pulled: String,
}

impl ReqConsequences {
    fn for_text(text: &str) -> Self {
        Self {
            met: copy::WHY.to_string(),
            drop: copy::drop_receipt(text, None),
            carry: copy::carry_receipt(text, false),
            carry_pulled: copy::carry_receipt(text, true),
        }
    }
}

/// Offer envelope on a requirement row. Numbering is assigned later;
/// `n` stays 0 until the shared card view exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReqOfferEnvelope {
    pub(crate) key: String,
    pub(crate) n: usize,
    pub(crate) group: &'static str,
    pub(crate) selected: bool,
}

/// One pending requirement row in a [`ReqCloseGroup`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReqCloseRow {
    pub(crate) offer: ReqOfferEnvelope,
    pub(crate) owner_key: String,
    pub(crate) path: String,
    pub(crate) depth: usize,
    pub(crate) parent_key: Option<String>,
    pub(crate) text: String,
    pub(crate) line: usize,
    pub(crate) state: &'static str,
    pub(crate) pending_descendants: usize,
    pub(crate) depth_exceeded: bool,
    pub(crate) run: &'static str,
    pub(crate) accessibility: String,
    pub(crate) consequences: ReqConsequences,
}

/// Canonical group copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReqGroupCopy {
    pub(crate) title: String,
    pub(crate) scope: String,
    pub(crate) why: &'static str,
    pub(crate) history: Option<String>,
    pub(crate) glyph: &'static str,
    pub(crate) footer: &'static str,
    pub(crate) hint: &'static str,
}

/// Historical fold counts on a requirements group. Zero fields stay 0
/// so later phases can omit them on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ReqHistoryFolds {
    pub(crate) dropped: usize,
    pub(crate) carried: usize,
    pub(crate) met: usize,
}

/// One `requirements` group for a closing owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReqCloseGroup {
    pub(crate) group: &'static str,
    pub(crate) kind: &'static str,
    pub(crate) owner: ReqCloseOwnerView,
    pub(crate) copy: ReqGroupCopy,
    pub(crate) folded: ReqHistoryFolds,
    pub(crate) rows: Vec<ReqCloseRow>,
}

/// Wire-facing owner identity carried on the group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReqCloseOwnerView {
    pub(crate) task_key: String,
    pub(crate) key: String,
    pub(crate) path: String,
    pub(crate) block_id: String,
    pub(crate) line: usize,
    pub(crate) text: String,
}

/// One recognized req inside a pre-close owner snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReqCloseNode {
    pub(crate) key: String,
    pub(crate) path: String,
    pub(crate) depth: usize,
    pub(crate) parent_key: Option<String>,
    pub(crate) text: String,
    pub(crate) line: usize,
    pub(crate) state: &'static str,
    pub(crate) mark: &'static str,
    pub(crate) pending_descendants: usize,
    pub(crate) depth_exceeded: bool,
}

/// Immutable pre-close forest for one managed Done owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReqCloseOwner {
    pub(crate) task_key: String,
    pub(crate) key: String,
    pub(crate) path: String,
    pub(crate) block_id: String,
    pub(crate) line: usize,
    pub(crate) text: String,
    /// Exact pre-close owner-block bytes, including the last line ending.
    pub(crate) preimage: String,
    /// Exact pre-close task line, without its line ending.
    pub(crate) task_line: String,
    pub(crate) depth_exceeded: bool,
    pub(crate) nodes: Vec<ReqCloseNode>,
}

impl ReqCloseOwner {
    pub(crate) fn pending_count(&self) -> usize {
        self.nodes
            .iter()
            .filter(|node| node.state == "pending")
            .count()
    }

    pub(crate) fn pending_branches(&self) -> usize {
        self.nodes
            .iter()
            .filter(|node| node.state == "pending" && node.depth == 1)
            .count()
    }

    pub(crate) fn history_folds(&self) -> ReqHistoryFolds {
        let mut folds = ReqHistoryFolds::default();
        for node in &self.nodes {
            match node.state {
                "dropped" => folds.dropped += 1,
                "carried" => folds.carried += 1,
                "met" => folds.met += 1,
                _ => {}
            }
        }
        folds
    }

    pub(crate) fn view(&self) -> ReqCloseOwnerView {
        ReqCloseOwnerView {
            task_key: self.task_key.clone(),
            key: self.key.clone(),
            path: self.path.clone(),
            block_id: self.block_id.clone(),
            line: self.line,
            text: self.text.clone(),
        }
    }

    /// One requirements group when this owner has pending rows.
    pub(crate) fn group(&self) -> Option<ReqCloseGroup> {
        let pending: Vec<&ReqCloseNode> = self
            .nodes
            .iter()
            .filter(|node| node.state == "pending")
            .collect();
        if pending.is_empty() {
            return None;
        }
        let folds = self.history_folds();
        let rows = pending
            .iter()
            .map(|node| ReqCloseRow {
                offer: ReqOfferEnvelope {
                    key: node.key.clone(),
                    n: 0,
                    group: "requirements",
                    selected: false,
                },
                owner_key: self.key.clone(),
                path: node.path.clone(),
                depth: node.depth,
                parent_key: node.parent_key.clone(),
                text: node.text.clone(),
                line: node.line,
                state: node.state,
                pending_descendants: node.pending_descendants,
                depth_exceeded: node.depth_exceeded,
                run: req_glyph(EffectiveReqState::Pending),
                accessibility: copy::accessibility(node.depth, "will be met"),
                consequences: ReqConsequences::for_text(&node.text),
            })
            .collect();
        Some(ReqCloseGroup {
            group: "requirements",
            kind: "requirements",
            owner: self.view(),
            copy: ReqGroupCopy {
                title: copy::group_title(&self.text),
                scope: copy::scope(pending.len(), self.pending_branches()),
                why: copy::WHY,
                history: copy::history(folds.dropped, folds.carried, folds.met),
                glyph: "requirement",
                footer: copy::FOOTER,
                hint: copy::HINT,
            },
            folded: folds,
            rows,
        })
    }
}

/// Groups for a closing set, preserving the explicit close order.
pub(crate) fn groups_from_owners(
    owners: &[ReqCloseOwner],
) -> Vec<ReqCloseGroup> {
    owners.iter().filter_map(ReqCloseOwner::group).collect()
}

/// Bind an owner key to the complete pre-close owner-block preimage.
pub(crate) fn owner_snapshot_key(task_key: &str, preimage: &str) -> String {
    let digest = Sha256::digest(preimage.as_bytes());
    format!("{task_key}@{}", hex::encode(digest))
}

/// Exact owner-block bytes: the task line through [`NoteTask::block_end`].
pub(crate) fn owner_block_preimage(contents: &str, task: &NoteTask) -> String {
    let spans = line_spans(contents);
    let start = if task.line_index == 0 {
        0
    } else {
        spans
            .get(task.line_index - 1)
            .map(|span| span.end)
            .unwrap_or(0)
    };
    let end = task.block_end.min(contents.len());
    if start >= end || start > contents.len() {
        return String::new();
    }
    contents[start..end].to_string()
}

fn digest_ordinal_for(
    contents: &str,
    task: &NoteTask,
    settings: &NoteTaskSettings,
) -> usize {
    if task.block_id.is_some() {
        return 1;
    }
    let scan = note_tasks::scan(contents, settings);
    let mut ordinal = 0;
    for other in scan.tasks() {
        if other.block_id.is_some() {
            continue;
        }
        if other.description == task.description {
            ordinal += 1;
        }
        if other.line_index == task.line_index {
            break;
        }
    }
    ordinal.max(1)
}

fn pending_descendant_count(forest: &[ReqNode], node: &ReqNode) -> usize {
    forest
        .iter()
        .filter(|other| {
            other.line_index > node.line_index
                && other.line_index <= node.subtree_end
                && other.owner_line == node.owner_line
                && other.state == EffectiveReqState::Pending
        })
        .count()
}

fn row_keys_by_line(
    owner_key: &str,
    forest: &[ReqNode],
) -> Vec<(usize, String)> {
    forest
        .iter()
        .map(|node| {
            (
                node.line_index,
                req_row_key(owner_key, &node.path, &node.text),
            )
        })
        .collect()
}

/// Collect the immutable pre-close forest for one closing owner.
///
/// Recurring owners are skipped. Depth above [`MAX_REQ_DEPTH`] is kept
/// on the snapshot (`depth_exceeded`) rather than truncated.
pub(crate) fn collect_owner(
    contents: &str,
    relative_path: &Path,
    task: &NoteTask,
    settings: &NoteTaskSettings,
) -> Option<ReqCloseOwner> {
    let spans = line_spans(contents);
    let task_line = spans
        .get(task.line_index)
        .map(|span| span.text.to_string())
        .unwrap_or_else(|| task.description.clone());
    if is_recurring_task_line(&task_line) {
        return None;
    }
    // Tree close looks up with an empty filter so every checkbox is
    // visible; the snapshot still uses the vault filter so owner text
    // and idless keys match `stable_task_key`.
    let scan = note_tasks::scan(contents, settings);
    let owned = scan
        .task_at(task.line_index)
        .cloned()
        .unwrap_or_else(|| task.clone());
    let task = &owned;
    let preimage = owner_block_preimage(contents, task);
    let ordinal = digest_ordinal_for(contents, task, settings);
    let task_key = stable_task_key(
        relative_path,
        task.block_id.as_deref(),
        &task.description,
        settings.global_filter.as_str(),
        ordinal,
    );
    let key = owner_snapshot_key(&task_key, &preimage);
    let forest = task_reqs::req_forest(contents, task, settings);
    let keys = row_keys_by_line(&key, &forest);
    let key_for = |line: usize| -> Option<String> {
        keys.iter()
            .find(|(index, _)| *index == line)
            .map(|(_, key)| key.clone())
    };
    let mut depth_exceeded = false;
    let mut nodes = Vec::with_capacity(forest.len());
    for node in &forest {
        if node.depth_exceeded || node.depth > MAX_REQ_DEPTH {
            depth_exceeded = true;
        }
        nodes.push(ReqCloseNode {
            key: key_for(node.line_index)
                .unwrap_or_else(|| req_row_key(&key, &node.path, &node.text)),
            path: req_path(node),
            depth: node.depth,
            parent_key: node.parent.and_then(key_for),
            text: node.text.clone(),
            line: node.line_index + 1,
            state: node.state.name(),
            mark: node.mark.name(),
            pending_descendants: pending_descendant_count(&forest, node),
            depth_exceeded: node.depth_exceeded,
        });
    }
    Some(ReqCloseOwner {
        task_key,
        key,
        path: relative_path
            .components()
            .filter_map(|component| component.as_os_str().to_str())
            .collect::<Vec<_>>()
            .join("/"),
        block_id: task.block_id.clone().unwrap_or_default(),
        line: task.line_index + 1,
        text: task.description.clone(),
        preimage,
        task_line,
        depth_exceeded,
        nodes,
    })
}

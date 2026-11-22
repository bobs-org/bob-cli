//! Pure requirement-close card numbering, choices, folds, and validation.
//!
//! This module consumes freshly collected [`ReqCloseOwner`] snapshots and
//! never writes requirement state. Capture adapters resolve CLI selectors
//! through [`ReqCardNumbering`], then revalidate the selected owner
//! preimages before planning writes and again at commit.

use std::collections::{BTreeMap, BTreeSet};

use super::req_close::{
    groups_from_owners, owner_snapshot_key, ReqCloseGroup, ReqCloseOwner,
    ReqCloseRow,
};

/// The four final outcomes accepted by the requirement close card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ReqVerdict {
    Met,
    Drop,
    Carry,
    CarryPulled,
}

impl ReqVerdict {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Met => "met",
            Self::Drop => "drop",
            Self::Carry => "carry",
            Self::CarryPulled => "carry_pulled",
        }
    }

    fn takes_branch(self) -> bool {
        matches!(self, Self::Drop | Self::Carry | Self::CarryPulled)
    }
}

/// One final choice supplied by a client. `selector` is either a current
/// probe number or the complete opaque requirement row key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReqVerdictRequest {
    pub(crate) selector: String,
    pub(crate) verdict: ReqVerdict,
    pub(crate) reason: Option<String>,
}

/// A row in the shared requirement-first card number space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReqCardRow {
    pub(crate) key: String,
    pub(crate) n: usize,
    pub(crate) kind: ReqCardRowKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReqCardRowKind {
    Requirement,
    TaskOffer,
}

/// Number lookup for one freshly built card. Empty requirement groups
/// preserve the existing task-offer-only numbering for legacy callers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ReqCardNumbering {
    pub(crate) rows: Vec<ReqCardRow>,
    by_key: BTreeMap<String, usize>,
    by_number: BTreeMap<usize, usize>,
}

impl ReqCardNumbering {
    pub(crate) fn new(
        groups: &[ReqCloseGroup],
        task_offer_keys: &[String],
    ) -> Result<Self, ReqVerdictError> {
        let mut result = Self::default();
        for group in groups {
            for row in &group.rows {
                result
                    .push(row.offer.key.clone(), ReqCardRowKind::Requirement)?;
            }
        }
        for key in task_offer_keys {
            result.push(key.clone(), ReqCardRowKind::TaskOffer)?;
        }
        Ok(result)
    }

    fn push(
        &mut self,
        key: String,
        kind: ReqCardRowKind,
    ) -> Result<(), ReqVerdictError> {
        let n = self.rows.len() + 1;
        if self.by_key.contains_key(&key) {
            return Err(ReqVerdictError::DuplicateCardKey { key });
        }
        self.by_key.insert(key.clone(), n);
        self.by_number.insert(n, self.rows.len());
        self.rows.push(ReqCardRow { key, n, kind });
        Ok(())
    }

    /// Resolve a complete opaque key or a number from this card's probe.
    pub(crate) fn resolve(
        &self,
        selector: &str,
    ) -> Result<&ReqCardRow, ReqVerdictError> {
        if selector.bytes().all(|byte| byte.is_ascii_digit()) {
            let row = selector
                .parse::<usize>()
                .ok()
                .and_then(|n| self.by_number.get(&n))
                .and_then(|index| self.rows.get(*index));
            return row.ok_or_else(|| ReqVerdictError::Stale {
                selectors: vec![selector.to_string()],
                current: None,
            });
        }
        self.by_key
            .get(selector)
            .and_then(|n| self.by_number.get(n))
            .and_then(|index| self.rows.get(*index))
            .ok_or_else(|| ReqVerdictError::Stale {
                selectors: vec![selector.to_string()],
                current: None,
            })
    }

    /// Copy the shared card numbers onto requirement group rows.
    pub(crate) fn numbered_groups(
        &self,
        groups: &[ReqCloseGroup],
    ) -> Result<Vec<ReqCloseGroup>, ReqVerdictError> {
        let mut groups = groups.to_vec();
        for group in &mut groups {
            for row in &mut group.rows {
                row.offer.n = self
                    .by_key
                    .get(row.offer.key.as_str())
                    .copied()
                    .ok_or_else(|| ReqVerdictError::Stale {
                        selectors: vec![row.offer.key.clone()],
                        current: None,
                    })?;
            }
        }
        Ok(groups)
    }

    /// Mark-all is defined over ordinary task offers only.
    pub(crate) fn mark_all_keys(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|row| row.kind == ReqCardRowKind::TaskOffer)
            .map(|row| row.key.clone())
            .collect()
    }
}

/// Immutable owner and row provenance retained beside a resolved choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReqRowProvenance {
    pub(crate) owner_task_key: String,
    pub(crate) owner_key: String,
    pub(crate) owner_path: String,
    pub(crate) owner_block_id: String,
    pub(crate) owner_line: usize,
    pub(crate) owner_preimage: String,
    pub(crate) key: String,
    pub(crate) path: String,
    pub(crate) depth: usize,
    pub(crate) parent_key: Option<String>,
    pub(crate) text: String,
    pub(crate) line: usize,
    pub(crate) pending_descendants: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedReqVerdict {
    pub(crate) row: ReqRowProvenance,
    pub(crate) verdict: ReqVerdict,
    pub(crate) reason: Option<String>,
}

/// A jump pin is navigation-only. Its target can be a requirement or an
/// ordinary offer; only requirement targets carry owner provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReqCardTarget {
    pub(crate) card_row: ReqCardRow,
    pub(crate) requirement: Option<ReqRowProvenance>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReqVerdictSelection {
    pub(crate) groups: Vec<ReqCloseGroup>,
    pub(crate) numbering: ReqCardNumbering,
    pub(crate) verdicts: Vec<ResolvedReqVerdict>,
    pub(crate) jump: Option<ReqCardTarget>,
}

/// Fresh recovery view attached to a stale-selector refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReqCurrentCard {
    pub(crate) groups: Vec<ReqCloseGroup>,
    pub(crate) rows: Vec<ReqCardRow>,
}

/// Portable, no-write staged UI state. A repeated verdict toggles the row
/// back to its default Met state. Descendant choices remain staged while
/// an ancestor hides them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ReqChoiceState {
    choices: BTreeMap<String, ReqVerdict>,
}

impl ReqChoiceState {
    pub(crate) fn choice(&self, key: &str) -> ReqVerdict {
        self.choices.get(key).copied().unwrap_or(ReqVerdict::Met)
    }

    pub(crate) fn stage(
        &mut self,
        key: impl Into<String>,
        verdict: ReqVerdict,
    ) {
        let key = key.into();
        if verdict == ReqVerdict::Met
            || self.choices.get(&key).copied() == Some(verdict)
        {
            self.choices.remove(&key);
        } else {
            self.choices.insert(key, verdict);
        }
    }

    /// Visible rows plus the folded child count shown on a structural
    /// verdict. Hidden child state is preserved in `self.choices`.
    pub(crate) fn fold(&self, group: &ReqCloseGroup) -> Vec<ReqRowFold> {
        let by_key: BTreeMap<&str, &ReqCloseRow> = group
            .rows
            .iter()
            .map(|row| (row.offer.key.as_str(), row))
            .collect();
        group
            .rows
            .iter()
            .map(|row| {
                let mut ancestor = row.parent_key.as_deref();
                let mut hidden_by = None;
                while let Some(key) = ancestor {
                    if self
                        .choices
                        .get(key)
                        .is_some_and(|choice| choice.takes_branch())
                    {
                        hidden_by = Some(key.to_string());
                        break;
                    }
                    ancestor = by_key
                        .get(key)
                        .and_then(|parent| parent.parent_key.as_deref());
                }
                let choice = self.choice(&row.offer.key);
                ReqRowFold {
                    key: row.offer.key.clone(),
                    visible: hidden_by.is_none(),
                    hidden_by,
                    nested_go_with_it: (choice.takes_branch()
                        && row.pending_descendants > 0)
                        .then_some(row.pending_descendants),
                }
            })
            .collect()
    }

    pub(crate) fn requests(&self) -> Vec<ReqVerdictRequest> {
        self.choices
            .iter()
            .map(|(key, verdict)| ReqVerdictRequest {
                selector: key.clone(),
                verdict: *verdict,
                reason: None,
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReqRowFold {
    pub(crate) key: String,
    pub(crate) visible: bool,
    pub(crate) hidden_by: Option<String>,
    pub(crate) nested_go_with_it: Option<usize>,
}

/// Refusal from the pure choice resolver. Capture maps these stable codes
/// to its structured `CaptureError` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReqVerdictError {
    Stale {
        selectors: Vec<String>,
        current: Option<ReqCurrentCard>,
    },
    DuplicateCardKey {
        key: String,
    },
    DuplicateOwner {
        task_key: String,
    },
    Conflict {
        key: String,
    },
    Overlap {
        ancestor_key: String,
        ancestor_text: String,
        descendant_key: String,
    },
    DepthExceeded {
        key: String,
        depth: usize,
    },
    InvalidReason {
        key: String,
    },
}

impl ReqVerdictError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Stale { .. }
            | Self::DuplicateCardKey { .. }
            | Self::DuplicateOwner { .. } => "req_close_stale",
            Self::Conflict { .. } => "offer_verdict_conflict",
            Self::Overlap { .. } => "req_verdict_overlap",
            Self::DepthExceeded { .. } => "req_depth_exceeded",
            Self::InvalidReason { .. } => "offer_verdict_conflict",
        }
    }
}

/// Resolve explicit requirement choices and one optional navigation pin
/// against freshly collected owners and this command's ordinary offers.
pub(crate) fn resolve_verdicts(
    owners: &[ReqCloseOwner],
    task_offer_keys: &[String],
    requests: &[ReqVerdictRequest],
    jump_selector: Option<&str>,
) -> Result<ReqVerdictSelection, ReqVerdictError> {
    validate_owner_identities(owners)?;
    let unnumbered_groups = groups_from_owners(owners);
    let numbering = ReqCardNumbering::new(&unnumbered_groups, task_offer_keys)?;
    let groups = numbering.numbered_groups(&unnumbered_groups)?;
    let rows = row_index(owners, &groups);
    let mut selected = Vec::<ResolvedReqVerdict>::new();
    let mut selected_by_key = BTreeMap::<String, usize>::new();
    let mut bad = Vec::new();

    for request in requests {
        let card_row = match numbering.resolve(&request.selector) {
            Ok(row) if row.kind == ReqCardRowKind::Requirement => row,
            Ok(_) => {
                bad.push(request.selector.clone());
                continue;
            }
            Err(_) => {
                bad.push(request.selector.clone());
                continue;
            }
        };
        let Some((owner, row)) = rows.get(card_row.key.as_str()).copied()
        else {
            bad.push(request.selector.clone());
            continue;
        };
        let reason = normalize_reason(request.reason.as_deref());
        if reason.is_some() && request.verdict != ReqVerdict::Drop {
            return Err(ReqVerdictError::InvalidReason {
                key: card_row.key.clone(),
            });
        }
        if request.verdict.takes_branch() && row.depth_exceeded {
            return Err(ReqVerdictError::DepthExceeded {
                key: card_row.key.clone(),
                depth: row.depth,
            });
        }
        let resolved = ResolvedReqVerdict {
            row: provenance(owner, row),
            verdict: request.verdict,
            reason,
        };
        if let Some(previous_index) = selected_by_key.get(&card_row.key) {
            let previous = &selected[*previous_index];
            if previous.verdict != resolved.verdict
                || previous.reason != resolved.reason
            {
                return Err(ReqVerdictError::Conflict {
                    key: card_row.key.clone(),
                });
            }
            // Identical repeated choices, including repeated carries, are
            // idempotent and retain the first request's order.
            continue;
        }
        selected_by_key.insert(card_row.key.clone(), selected.len());
        selected.push(resolved);
    }
    if !bad.is_empty() {
        return Err(stale_current(bad, &groups, &numbering));
    }
    validate_non_overlapping(&selected, &rows)?;

    let jump = if let Some(selector) = jump_selector {
        let card_row = match numbering.resolve(selector) {
            Ok(row) => row,
            Err(_) => {
                return Err(stale_current(
                    vec![selector.to_string()],
                    &groups,
                    &numbering,
                ));
            }
        };
        let requirement = if card_row.kind == ReqCardRowKind::Requirement {
            rows.get(card_row.key.as_str())
                .map(|(owner, row)| provenance(owner, row))
        } else {
            None
        };
        Some(ReqCardTarget {
            card_row: card_row.clone(),
            requirement,
        })
    } else {
        None
    };

    Ok(ReqVerdictSelection {
        groups,
        numbering,
        verdicts: selected,
        jump,
    })
}

/// Compare a probe selection with the freshly collected final batch. Exact
/// pre-close bytes and row identity must still match before any planned
/// requirement write is accepted.
pub(crate) fn revalidate_preimages(
    selection: &ReqVerdictSelection,
    fresh_owners: &[ReqCloseOwner],
    fresh_task_offer_keys: &[String],
) -> Result<(), ReqVerdictError> {
    validate_owner_identities(fresh_owners)?;
    let fresh_groups = groups_from_owners(fresh_owners);
    let fresh_numbering =
        ReqCardNumbering::new(&fresh_groups, fresh_task_offer_keys)?;
    let fresh_numbered_groups =
        fresh_numbering.numbered_groups(&fresh_groups)?;
    let fresh_by_task: BTreeMap<&str, &ReqCloseOwner> = fresh_owners
        .iter()
        .map(|owner| (owner.task_key.as_str(), owner))
        .collect();
    let mut checked = BTreeSet::new();
    let mut rows_to_check: Vec<&ReqRowProvenance> = selection
        .verdicts
        .iter()
        .map(|verdict| &verdict.row)
        .collect();
    if let Some(row) = selection
        .jump
        .as_ref()
        .and_then(|target| target.requirement.as_ref())
    {
        rows_to_check.push(row);
    }
    for selected in rows_to_check {
        if !checked.insert(selected.owner_task_key.as_str()) {
            continue;
        }
        let Some(fresh) = fresh_by_task.get(selected.owner_task_key.as_str())
        else {
            return Err(stale_current(
                vec![selected.key.clone()],
                &fresh_numbered_groups,
                &fresh_numbering,
            ));
        };
        if fresh.key != selected.owner_key
            || fresh.path != selected.owner_path
            || fresh.block_id != selected.owner_block_id
            || fresh.line != selected.owner_line
            || fresh.preimage != selected.owner_preimage
            || owner_snapshot_key(&fresh.task_key, &fresh.preimage) != fresh.key
            || !fresh.nodes.iter().any(|node| {
                node.key == selected.key
                    && node.path == selected.path
                    && node.depth == selected.depth
                    && node.parent_key == selected.parent_key
                    && node.text == selected.text
                    && node.line == selected.line
                    && node.state == "pending"
            })
        {
            return Err(stale_current(
                vec![selected.key.clone()],
                &fresh_numbered_groups,
                &fresh_numbering,
            ));
        }
    }
    Ok(())
}

fn validate_owner_identities(
    owners: &[ReqCloseOwner],
) -> Result<(), ReqVerdictError> {
    let mut task_keys = BTreeSet::new();
    let mut owner_keys = BTreeSet::new();
    for owner in owners {
        if !task_keys.insert(owner.task_key.as_str())
            || !owner_keys.insert(owner.key.as_str())
            || owner_snapshot_key(&owner.task_key, &owner.preimage) != owner.key
        {
            return Err(ReqVerdictError::DuplicateOwner {
                task_key: owner.task_key.clone(),
            });
        }
    }
    Ok(())
}

fn stale_current(
    selectors: Vec<String>,
    groups: &[ReqCloseGroup],
    numbering: &ReqCardNumbering,
) -> ReqVerdictError {
    let groups = numbering
        .numbered_groups(groups)
        .unwrap_or_else(|_| groups.to_vec());
    ReqVerdictError::Stale {
        selectors,
        current: Some(ReqCurrentCard {
            groups,
            rows: numbering.rows.clone(),
        }),
    }
}

fn row_index<'a>(
    owners: &'a [ReqCloseOwner],
    groups: &'a [ReqCloseGroup],
) -> BTreeMap<&'a str, (&'a ReqCloseOwner, &'a ReqCloseRow)> {
    let owners_by_key: BTreeMap<&str, &ReqCloseOwner> = owners
        .iter()
        .map(|owner| (owner.key.as_str(), owner))
        .collect();
    let mut rows = BTreeMap::new();
    for group in groups {
        let Some(owner) = owners_by_key.get(group.owner.key.as_str()).copied()
        else {
            continue;
        };
        for row in &group.rows {
            rows.insert(row.offer.key.as_str(), (owner, row));
        }
    }
    rows
}

fn provenance(owner: &ReqCloseOwner, row: &ReqCloseRow) -> ReqRowProvenance {
    ReqRowProvenance {
        owner_task_key: owner.task_key.clone(),
        owner_key: owner.key.clone(),
        owner_path: owner.path.clone(),
        owner_block_id: owner.block_id.clone(),
        owner_line: owner.line,
        owner_preimage: owner.preimage.clone(),
        key: row.offer.key.clone(),
        path: row.path.clone(),
        depth: row.depth,
        parent_key: row.parent_key.clone(),
        text: row.text.clone(),
        line: row.line,
        pending_descendants: row.pending_descendants,
    }
}

fn validate_non_overlapping(
    selections: &[ResolvedReqVerdict],
    rows: &BTreeMap<&str, (&ReqCloseOwner, &ReqCloseRow)>,
) -> Result<(), ReqVerdictError> {
    for ancestor in selections
        .iter()
        .filter(|selection| selection.verdict.takes_branch())
    {
        for descendant in selections {
            if descendant.row.owner_key != ancestor.row.owner_key
                || descendant.row.key == ancestor.row.key
            {
                continue;
            }
            let mut parent = descendant.row.parent_key.as_deref();
            while let Some(parent_key) = parent {
                if parent_key == ancestor.row.key {
                    return Err(ReqVerdictError::Overlap {
                        ancestor_key: ancestor.row.key.clone(),
                        ancestor_text: ancestor.row.text.clone(),
                        descendant_key: descendant.row.key.clone(),
                    });
                }
                parent = rows
                    .get(parent_key)
                    .and_then(|(_, row)| row.parent_key.as_deref());
            }
        }
    }
    Ok(())
}

fn normalize_reason(reason: Option<&str>) -> Option<String> {
    reason.and_then(|reason| {
        let normalized =
            reason.split_whitespace().collect::<Vec<_>>().join(" ");
        (!normalized.is_empty()).then_some(normalized)
    })
}

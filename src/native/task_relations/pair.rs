//! Pure pair-state and pair-write planner (`docs/task-relations.md` §8).
//!
//! A pair is two distinct tasks `{A, B}` with at most one record per
//! ordered pair. `Here` is the owner (the captured, Task Card, or
//! calling task, usually N) and `There` is the other task (usually T).
//! The planner never mints block ids and never touches the vault; the
//! caller applies the returned LINKS operations through
//! `plan_links_edit` and the mirror deltas through the existing mirror
//! path.

// The write planner is shared by the follow-on Rust phases (capture
// uses Link and State) and the JS twin (ClearLabel, Swap,
// RemoveStatement, and Unlink exist for parity and are pinned by the
// same LW vectors), so not every item has a Rust caller yet.
#![allow(dead_code)]

/// Which side of a pair a value belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    Here,
    There,
}

impl Side {
    pub(crate) fn opposite(self) -> Side {
        match self {
            Side::Here => Side::There,
            Side::There => Side::Here,
        }
    }
}

/// One ordered-pair record slot: absent, plain, or labelled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecordKind {
    Absent,
    Plain,
    Labelled(String),
}

impl RecordKind {
    fn label(&self) -> Option<String> {
        match self {
            RecordKind::Labelled(label) => Some(label.clone()),
            _ => None,
        }
    }

    fn is_present(&self) -> bool {
        !matches!(self, RecordKind::Absent)
    }

    fn is_plain(&self) -> bool {
        matches!(self, RecordKind::Plain)
    }

    fn is_labelled(&self) -> bool {
        matches!(self, RecordKind::Labelled(_))
    }
}

/// Why a side cannot be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Writability {
    Writable,
    Archived,
    PreviousDaily,
}

impl Writability {
    pub(crate) fn is_writable(self) -> bool {
        matches!(self, Writability::Writable)
    }

    fn refusal_message(self) -> &'static str {
        match self {
            Writability::Writable => "the link is writable",
            Writability::Archived => "the link is stored on an archived task",
            Writability::PreviousDaily => {
                "the link is stored on the previous daily note"
            }
        }
    }
}

/// Both records of one pair plus per-side writability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PairRecords {
    pub(crate) here: RecordKind,
    pub(crate) here_writability: Writability,
    pub(crate) there: RecordKind,
    pub(crate) there_writability: Writability,
}

impl PairRecords {
    fn get(&self, side: Side) -> &RecordKind {
        match side {
            Side::Here => &self.here,
            Side::There => &self.there,
        }
    }

    fn writability(&self, side: Side) -> Writability {
        match side {
            Side::Here => self.here_writability,
            Side::There => self.there_writability,
        }
    }
}

/// Derived meaning of one pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PairState {
    Unrelated,
    Undirected {
        here_plain: bool,
        there_plain: bool,
    },
    Directed {
        here_label: Option<String>,
        there_label: Option<String>,
        redundant_here: bool,
        redundant_there: bool,
    },
}

/// Derive the pair meaning from its two records.
pub(crate) fn pair_state(records: &PairRecords) -> PairState {
    let here_label = records.here.label();
    let there_label = records.there.label();
    let has_labelled = here_label.is_some() || there_label.is_some();
    let has_record = records.here.is_present() || records.there.is_present();
    if !has_record {
        return PairState::Unrelated;
    }
    if !has_labelled {
        return PairState::Undirected {
            here_plain: records.here.is_plain(),
            there_plain: records.there.is_plain(),
        };
    }
    PairState::Directed {
        here_label,
        there_label,
        redundant_here: records.here.is_plain() && records.there.is_present(),
        redundant_there: records.there.is_plain() && records.here.is_present(),
    }
}

/// One pair-write request. `Here` is the owner (N).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PairRequest {
    /// Plain link N to T.
    Link,
    /// Labelled statement with an explicit source side.
    State { source: Side, label: String },
    /// Clear the label on the source side's statement.
    ClearLabel { source: Side },
    /// Move statement `label` from `from` to the opposite side.
    Swap { from: Side, label: String },
    /// Remove the statement authored by `source`.
    RemoveStatement { source: Side },
    /// Remove every writable record of the pair.
    Unlink,
}

/// One LINKS operation for one side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LinksOp {
    Add { label: Option<String> },
    Relabel { label: Option<String> },
    Remove,
}

/// One LINKED FROM delta for one side's mirror line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MirrorDelta {
    Keep,
    Upsert { label: Option<String> },
    Remove,
}

/// What the planner did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PairAction {
    Added,
    Relabeled,
    Unchanged,
    Removed,
}

/// Resulting pair kind. `None` means unrelated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PairKind {
    Undirected,
    Directed,
}

/// Pure plan for one pair write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PairPlan {
    pub(crate) here_op: Option<LinksOp>,
    pub(crate) there_op: Option<LinksOp>,
    /// Delta to Here's LINKED FROM (lists There when visible).
    pub(crate) here_mirror: MirrorDelta,
    /// Delta to There's LINKED FROM (lists Here when visible).
    pub(crate) there_mirror: MirrorDelta,
    pub(crate) action: PairAction,
    pub(crate) kind: Option<PairKind>,
    pub(crate) moved_from: Option<Side>,
    pub(crate) refusal: Option<String>,
}

impl PairPlan {
    #[cfg(test)]
    fn op_for(&self, side: Side) -> &Option<LinksOp> {
        match side {
            Side::Here => &self.here_op,
            Side::There => &self.there_op,
        }
    }
}

/// Projection restricted to this pair: the mirror entry `side` shows
/// for the opposite side's record. Outer `None` means no entry;
/// inner `None` means a plain entry, `Some` a labelled entry.
pub(crate) fn mirror_entry(
    records: &PairRecords,
    side: Side,
) -> Option<Option<String>> {
    let (own, other) = match side {
        Side::Here => (&records.here, &records.there),
        Side::There => (&records.there, &records.here),
    };
    match other {
        RecordKind::Absent => None,
        RecordKind::Labelled(label) => Some(Some(label.clone())),
        RecordKind::Plain => {
            if own.is_present() {
                None
            } else {
                Some(None)
            }
        }
    }
}

fn apply_op(record: &RecordKind, op: &Option<LinksOp>) -> RecordKind {
    match op {
        None => record.clone(),
        Some(LinksOp::Add { label }) | Some(LinksOp::Relabel { label }) => {
            match label {
                None => RecordKind::Plain,
                Some(value) => RecordKind::Labelled(value.clone()),
            }
        }
        Some(LinksOp::Remove) => RecordKind::Absent,
    }
}

fn after_records(
    records: &PairRecords,
    here_op: &Option<LinksOp>,
    there_op: &Option<LinksOp>,
) -> PairRecords {
    PairRecords {
        here: apply_op(&records.here, here_op),
        here_writability: records.here_writability,
        there: apply_op(&records.there, there_op),
        there_writability: records.there_writability,
    }
}

fn mirror_delta(
    before: Option<Option<String>>,
    after: Option<Option<String>>,
) -> MirrorDelta {
    if before == after {
        return MirrorDelta::Keep;
    }
    match after {
        None => MirrorDelta::Remove,
        Some(label) => MirrorDelta::Upsert { label },
    }
}

fn deltas_for(
    before: &PairRecords,
    here_op: &Option<LinksOp>,
    there_op: &Option<LinksOp>,
) -> (MirrorDelta, MirrorDelta) {
    let after = after_records(before, here_op, there_op);
    (
        mirror_delta(
            mirror_entry(before, Side::Here),
            mirror_entry(&after, Side::Here),
        ),
        mirror_delta(
            mirror_entry(before, Side::There),
            mirror_entry(&after, Side::There),
        ),
    )
}

fn resulting_kind(records: &PairRecords) -> Option<PairKind> {
    match pair_state(records) {
        PairState::Unrelated => None,
        PairState::Undirected { .. } => Some(PairKind::Undirected),
        PairState::Directed { .. } => Some(PairKind::Directed),
    }
}

fn refused_plan(
    records: &PairRecords,
    here_op: Option<LinksOp>,
    there_op: Option<LinksOp>,
    action: PairAction,
    kind: Option<PairKind>,
    reason: Writability,
) -> PairPlan {
    let _ = (records, here_op, there_op);
    PairPlan {
        here_op: None,
        there_op: None,
        here_mirror: MirrorDelta::Keep,
        there_mirror: MirrorDelta::Keep,
        action,
        kind,
        moved_from: None,
        refusal: Some(reason.refusal_message().to_string()),
    }
}

fn refused_logical(
    here_op: Option<LinksOp>,
    there_op: Option<LinksOp>,
    action: PairAction,
    kind: Option<PairKind>,
    message: impl Into<String>,
) -> PairPlan {
    let _ = (here_op, there_op);
    PairPlan {
        here_op: None,
        there_op: None,
        here_mirror: MirrorDelta::Keep,
        there_mirror: MirrorDelta::Keep,
        action,
        kind,
        moved_from: None,
        refusal: Some(message.into()),
    }
}

fn completed_plan(
    before: &PairRecords,
    here_op: Option<LinksOp>,
    there_op: Option<LinksOp>,
    action: PairAction,
    moved_from: Option<Side>,
) -> PairPlan {
    let (here_mirror, there_mirror) = deltas_for(before, &here_op, &there_op);
    let after = after_records(before, &here_op, &there_op);
    PairPlan {
        here_op,
        there_op,
        here_mirror,
        there_mirror,
        action,
        kind: resulting_kind(&after),
        moved_from,
        refusal: None,
    }
}

/// Plan one pair write. Never mints and never refuses when nothing
/// needs to change.
pub(crate) fn plan_pair_write(
    records: &PairRecords,
    request: &PairRequest,
) -> PairPlan {
    match request {
        PairRequest::Link => plan_link(records),
        PairRequest::State { source, label } => {
            plan_state(records, *source, label)
        }
        PairRequest::ClearLabel { source } => {
            plan_clear_label(records, *source)
        }
        PairRequest::Swap { from, label } => plan_swap(records, *from, label),
        PairRequest::RemoveStatement { source } => {
            plan_remove_statement(records, *source)
        }
        PairRequest::Unlink => plan_unlink(records),
    }
}

fn plan_link(records: &PairRecords) -> PairPlan {
    if records.here.is_present() || records.there.is_present() {
        let kind = resulting_kind(records);
        let (here_mirror, there_mirror) =
            (MirrorDelta::Keep, MirrorDelta::Keep);
        return PairPlan {
            here_op: None,
            there_op: None,
            here_mirror,
            there_mirror,
            action: PairAction::Unchanged,
            kind,
            moved_from: None,
            refusal: None,
        };
    }
    if records.here_writability.is_writable() {
        let here_op = Some(LinksOp::Add { label: None });
        return completed_plan(records, here_op, None, PairAction::Added, None);
    }
    if records.there_writability.is_writable() {
        let there_op = Some(LinksOp::Add { label: None });
        return completed_plan(
            records,
            None,
            there_op,
            PairAction::Added,
            None,
        );
    }
    refused_plan(
        records,
        Some(LinksOp::Add { label: None }),
        None,
        PairAction::Added,
        None,
        records.here_writability,
    )
}

fn plan_state(records: &PairRecords, source: Side, label: &str) -> PairPlan {
    let target = source.opposite();
    let src_record = records.get(source).clone();
    let dst_record = records.get(target).clone();

    if let RecordKind::Labelled(existing) = &src_record
        && existing == label
    {
        let kind = resulting_kind(records);
        return PairPlan {
            here_op: None,
            there_op: None,
            here_mirror: MirrorDelta::Keep,
            there_mirror: MirrorDelta::Keep,
            action: PairAction::Unchanged,
            kind,
            moved_from: None,
            refusal: None,
        };
    }

    if !records.writability(source).is_writable() {
        let kind = resulting_kind(records);
        return refused_plan(
            records,
            None,
            None,
            PairAction::Relabeled,
            kind,
            records.writability(source),
        );
    }

    let src_op = match &src_record {
        RecordKind::Absent => Some(LinksOp::Add {
            label: Some(label.to_string()),
        }),
        RecordKind::Plain | RecordKind::Labelled(_) => Some(LinksOp::Relabel {
            label: Some(label.to_string()),
        }),
    };
    let dst_op = match &dst_record {
        RecordKind::Plain if records.writability(target).is_writable() => {
            Some(LinksOp::Remove)
        }
        _ => None,
    };
    let moved_from = match &dst_op {
        Some(LinksOp::Remove) => Some(target),
        _ => None,
    };
    let action = match (&src_record, &dst_record) {
        (RecordKind::Absent, RecordKind::Absent) => PairAction::Added,
        (RecordKind::Absent, RecordKind::Labelled(_)) => PairAction::Added,
        _ => PairAction::Relabeled,
    };
    let (here_op, there_op) = match source {
        Side::Here => (src_op, dst_op),
        Side::There => (dst_op, src_op),
    };
    completed_plan(records, here_op, there_op, action, moved_from)
}

fn plan_clear_label(records: &PairRecords, source: Side) -> PairPlan {
    let target = source.opposite();
    let src_record = records.get(source).clone();
    let dst_record = records.get(target).clone();

    let RecordKind::Labelled(_) = src_record else {
        let kind = resulting_kind(records);
        return PairPlan {
            here_op: None,
            there_op: None,
            here_mirror: MirrorDelta::Keep,
            there_mirror: MirrorDelta::Keep,
            action: PairAction::Unchanged,
            kind,
            moved_from: None,
            refusal: None,
        };
    };

    if !records.writability(source).is_writable() {
        let kind = resulting_kind(records);
        return refused_plan(
            records,
            None,
            None,
            PairAction::Relabeled,
            kind,
            records.writability(source),
        );
    }

    if dst_record.is_labelled() {
        let src_op = Some(LinksOp::Remove);
        let (here_op, there_op) = match source {
            Side::Here => (src_op, None),
            Side::There => (None, src_op),
        };
        return completed_plan(
            records,
            here_op,
            there_op,
            PairAction::Removed,
            None,
        );
    }

    let src_op = Some(LinksOp::Relabel { label: None });
    let dst_op = match &dst_record {
        RecordKind::Plain if records.writability(target).is_writable() => {
            Some(LinksOp::Remove)
        }
        _ => None,
    };
    let moved_from = match &dst_op {
        Some(LinksOp::Remove) => Some(target),
        _ => None,
    };
    let (here_op, there_op) = match source {
        Side::Here => (src_op, dst_op),
        Side::There => (dst_op, src_op),
    };
    completed_plan(
        records,
        here_op,
        there_op,
        PairAction::Relabeled,
        moved_from,
    )
}

fn plan_swap(records: &PairRecords, from: Side, label: &str) -> PairPlan {
    let to = from.opposite();
    let from_record = records.get(from).clone();
    let to_record = records.get(to).clone();

    let matches = match &from_record {
        RecordKind::Labelled(existing) => existing == label,
        _ => false,
    };
    if !matches {
        let kind = resulting_kind(records);
        return refused_logical(
            None,
            None,
            PairAction::Relabeled,
            kind,
            "no such statement to swap",
        );
    }
    if to_record.is_labelled() {
        let kind = resulting_kind(records);
        return refused_logical(
            None,
            None,
            PairAction::Relabeled,
            kind,
            "a reverse statement already exists",
        );
    }
    if !records.writability(to).is_writable() {
        let kind = resulting_kind(records);
        return refused_plan(
            records,
            None,
            None,
            PairAction::Relabeled,
            kind,
            records.writability(to),
        );
    }
    if !records.writability(from).is_writable() {
        let kind = resulting_kind(records);
        return refused_plan(
            records,
            None,
            None,
            PairAction::Relabeled,
            kind,
            records.writability(from),
        );
    }

    let to_op = match &to_record {
        RecordKind::Absent => Some(LinksOp::Add {
            label: Some(label.to_string()),
        }),
        RecordKind::Plain => Some(LinksOp::Relabel {
            label: Some(label.to_string()),
        }),
        RecordKind::Labelled(_) => None,
    };
    let from_op = Some(LinksOp::Remove);
    let (here_op, there_op) = match from {
        Side::Here => (from_op, to_op),
        Side::There => (to_op, from_op),
    };
    completed_plan(
        records,
        here_op,
        there_op,
        PairAction::Relabeled,
        Some(from),
    )
}

fn plan_remove_statement(records: &PairRecords, source: Side) -> PairPlan {
    let target = source.opposite();
    let src_record = records.get(source).clone();
    let dst_record = records.get(target).clone();

    if !src_record.is_labelled() {
        let kind = resulting_kind(records);
        return PairPlan {
            here_op: None,
            there_op: None,
            here_mirror: MirrorDelta::Keep,
            there_mirror: MirrorDelta::Keep,
            action: PairAction::Unchanged,
            kind,
            moved_from: None,
            refusal: None,
        };
    }
    if !records.writability(source).is_writable() {
        let kind = resulting_kind(records);
        return refused_plan(
            records,
            None,
            None,
            PairAction::Removed,
            kind,
            records.writability(source),
        );
    }

    let src_op = Some(LinksOp::Remove);
    let dst_op = match &dst_record {
        RecordKind::Labelled(_) => None,
        RecordKind::Plain if records.writability(target).is_writable() => {
            Some(LinksOp::Remove)
        }
        _ => None,
    };
    let (here_op, there_op) = match source {
        Side::Here => (src_op, dst_op),
        Side::There => (dst_op, src_op),
    };
    completed_plan(records, here_op, there_op, PairAction::Removed, None)
}

fn plan_unlink(records: &PairRecords) -> PairPlan {
    let here_present = records.here.is_present();
    let there_present = records.there.is_present();
    if !here_present && !there_present {
        return PairPlan {
            here_op: None,
            there_op: None,
            here_mirror: MirrorDelta::Keep,
            there_mirror: MirrorDelta::Keep,
            action: PairAction::Unchanged,
            kind: None,
            moved_from: None,
            refusal: None,
        };
    }
    let here_removable = here_present && records.here_writability.is_writable();
    let there_removable =
        there_present && records.there_writability.is_writable();
    if !here_removable && !there_removable {
        let reason = if here_present {
            records.here_writability
        } else {
            records.there_writability
        };
        let kind = resulting_kind(records);
        return refused_plan(
            records,
            None,
            None,
            PairAction::Removed,
            kind,
            reason,
        );
    }
    let here_op = here_removable.then_some(LinksOp::Remove);
    let there_op = there_removable.then_some(LinksOp::Remove);
    completed_plan(records, here_op, there_op, PairAction::Removed, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn writable_pair(
        here: RecordKind,
        there: RecordKind,
    ) -> PairRecords {
        PairRecords {
            here,
            here_writability: Writability::Writable,
            there,
            there_writability: Writability::Writable,
        }
    }
}

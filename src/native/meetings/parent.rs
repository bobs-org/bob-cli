//! Filing-parent resolution for a meeting row.
//!
//! Without `-P`, the parent is the event's `[root::]`, resolved by
//! [`parent_notes::resolve_parent`]. A missing, unresolvable, inbox, or
//! otherwise ineligible root is not an error here: it becomes
//! [`MeetingParent::NeedsParent`] so the CLI can ask once (and re-root
//! the event from the answer).

use std::path::Path;

use crate::native::parent_notes::{
    parent_candidates, resolve_parent, ParentCandidate, ParentError,
    ResolvedParent,
};

use super::model::MeetingRow;

/// Where the meeting's notes file: resolved, or awaiting an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MeetingParent {
    /// The notes file under this parent.
    Ready(ResolvedParent),
    /// The caller must ask: the event's root is missing or unusable.
    NeedsParent {
        /// The event's raw root value, when it names one.
        root: Option<String>,
        /// The notes the answer may choose from.
        candidates: Vec<ParentCandidate>,
    },
}

/// Resolve the filing parent for one meeting row.
///
/// `explicit` is the `-P` value; without it the row's `[root::]` wins.
/// An inbox root ([`is_inbox_note`](crate::native::task_complete::successors::is_inbox_note),
/// which includes `meet_inbox`) needs a parent exactly like a missing one.
pub(crate) fn meeting_parent(
    vault: &Path,
    row: &MeetingRow,
    explicit: Option<&str>,
) -> Result<MeetingParent, ParentError> {
    if let Some(input) = explicit {
        return resolve_parent(vault, input).map(MeetingParent::Ready);
    }
    let candidates = parent_candidates(vault);
    let Some(root) = row.root.as_deref() else {
        return Ok(MeetingParent::NeedsParent {
            root: None,
            candidates,
        });
    };
    let resolved = match resolve_parent(vault, root) {
        Ok(resolved) => resolved,
        Err(_) => {
            return Ok(MeetingParent::NeedsParent {
                root: Some(root.to_string()),
                candidates,
            });
        }
    };
    if resolved.kind == crate::native::capture_targets::CaptureTargetKind::Inbox
        || row.root_is_inbox
    {
        return Ok(MeetingParent::NeedsParent {
            root: Some(root.to_string()),
            candidates,
        });
    }
    Ok(MeetingParent::Ready(resolved))
}

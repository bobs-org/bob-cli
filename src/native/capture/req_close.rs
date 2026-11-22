//! Capture adapter for pre-close requirement forests.
//!
//! Tree close already binds each managed Done owner to its pre-close
//! snapshot. This adapter copies those owners onto capture close
//! results so successor pre-gates cannot drop a req-only audit, and
//! so req-less closes still add no extra vault scan.

use crate::native::task_complete::req_close::{
    groups_from_owners, ReqCloseGroup, ReqCloseOwner,
};

/// Requirement groups for a closing set, in the owners' close order.
pub(super) fn groups_for_close(owners: &[ReqCloseOwner]) -> Vec<ReqCloseGroup> {
    groups_from_owners(owners)
}

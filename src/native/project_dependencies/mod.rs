//! Project recognition and blocking derived from the `^prj` task line
//! (`docs/task-dependencies.md` §13).

mod blocking;
mod tree;

#[cfg(test)]
mod tests;

pub(crate) use blocking::{
    blocking_open_dependency_ids, effective_block_reasons,
    filter_internal_project_ids, is_project_task_id, project_block_reasons,
    BlockReason, ProjectBlockReasons, ProjectContext,
};
#[allow(unused_imports)]
// `ancestor_chain` is consumed by later cascade phases.
pub(crate) use tree::{
    ancestor_chain, project_ancestors, project_parent_target,
    resolve_project_parent, ProjectTree,
};

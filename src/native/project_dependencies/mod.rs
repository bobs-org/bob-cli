//! Project recognition and blocking derived from the `^prj` task line
//! (`docs/task-dependencies.md` §13).

mod blocking;

#[cfg(test)]
mod tests;

pub(crate) use blocking::{
    effective_block_reasons, project_block_reasons, BlockReason,
    ProjectBlockReasons, ProjectContext,
};

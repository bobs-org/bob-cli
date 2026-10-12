//! Shared project-dependency model (`docs/task-dependencies.md` §13).
//!
//! A project's (`^prj`) dependency links live in its note's `depends_on`
//! frontmatter list. This module owns the pure readers and writers
//! every engine shares; it changes no vault state itself:
//!
//! - [`parse`]: the `depends_on` frontmatter reader with the §13.11
//!   PDP verdicts;
//! - [`format`]: the key-span text splice with the §13.12 PDW
//!   guarantees, plus the writer link form;
//! - [`blocking`]: project recognition, the reusable openness maps,
//!   and the pure project/effective block reasons (§13.3).

pub(crate) mod blocking;
pub(crate) mod format;
pub(crate) mod parse;

#[cfg(test)]
mod tests;

pub(crate) use blocking::{
    effective_block_reasons, project_block_reasons, resolve_dependencies,
    BlockReason, Openness, ProjectBlockReasons, ProjectContext,
    ResolvedDependency,
};
pub(crate) use format::{format_project_link, splice_depends_on};
pub(crate) use parse::{
    parse_depends_on, DependsOnItem, DependsOnProperty, DEPENDS_ON_KEY,
};

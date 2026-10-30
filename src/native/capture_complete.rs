/// Cursor-aware completion for in-progress capture drafts.
/// Small facade; real logic lives in the child modules.
mod cancel;
mod candidates;
mod cli;
mod complete;
mod engine;
mod model;
mod pomodoros;
mod render;
mod shell;
mod support;
mod task_types;
#[cfg(test)]
mod tests;
mod theme_menu;

pub(super) const COMMAND_NAME: &str = "bob capture-complete";

pub(crate) use cli::{build_cli, run};
pub(crate) use shell::shell_completion;

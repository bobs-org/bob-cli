//! The dynamic completion engine.
//!
//! This is the only module that imports `clap_complete`. It wraps
//! `engine::complete` behind bob-owned candidate structs, so the rest of
//! completion never touches upstream types.
//!
//! Option-name slots are built from clap's public introspection instead:
//! the dynamic engine de-duplicates candidates by argument id, which
//! drops every short flag once its long form is listed, and bob needs
//! the paired `-b` / `--bob-dir` rows. Value and subcommand slots go
//! through the engine with a slot-normalized current word (empty for
//! values and commands, `--opt=` with an empty value for attached
//! values), so upstream prefix filtering never narrows a slot.
//!
//! Fallback: if upstream breaks, replace this file with an in-house
//! walker over clap's public introspection behind the same interface.

use std::ffi::OsString;

/// A shell-agnostic completion candidate with owned data.
#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    /// The literal value proposed for completion.
    pub value: OsString,
    /// Help text for the value, if any.
    pub help: Option<String>,
    /// De-duplication id (`arg::<id>` / `command::<name>`).
    pub id: Option<String>,
    /// Sort weight within a tag.
    pub order: usize,
    /// Whether the candidate is hidden.
    pub hidden: bool,
}

/// Slot-normalized dynamic completion for one cursor position.
///
/// `args` are the full words with the binary name first, `arg_index` the
/// cursor word's index. The caller passes a slot-normalized current word;
/// see the module docs. Upstream errors become empty output.
pub(crate) fn complete(
    command: &mut clap::Command,
    args: Vec<OsString>,
    arg_index: usize,
) -> Vec<Candidate> {
    match clap_complete::engine::complete(command, args, arg_index, None) {
        Ok(candidates) => candidates
            .into_iter()
            .map(|candidate| Candidate {
                value: candidate.get_value().to_os_string(),
                help: candidate.get_help().map(|help| help.to_string()),
                id: candidate.get_id().cloned(),
                order: candidate.get_display_order().unwrap_or(usize::MAX),
                hidden: candidate.is_hide_set(),
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Every option name of one command from public introspection, in help
/// order: long forms (`--name`, visible aliases) then, per argument,
/// short flags (`-x`) carrying the same help text, so the presenter can
/// render the paired rows rule 2 needs.
pub(crate) fn option_candidates(command: &clap::Command) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    for arg in command.get_arguments() {
        let help = arg.get_help().map(|help| help.to_string());
        let id = Some(format!("arg::{}", arg.get_id()));
        let order = arg.get_display_order();
        let hidden = arg.is_hide_set();
        if let Some(longs) = arg.get_long_and_visible_aliases() {
            for long in longs {
                candidates.push(Candidate {
                    value: OsString::from(format!("--{long}")),
                    help: help.clone(),
                    id: id.clone(),
                    order,
                    hidden,
                });
            }
        }
        if let Some(shorts) = arg.get_short_and_visible_aliases() {
            for short in shorts {
                candidates.push(Candidate {
                    value: OsString::from(format!("-{short}")),
                    help: help.clone(),
                    id: id.clone(),
                    order,
                    hidden,
                });
            }
        }
    }
    candidates
}

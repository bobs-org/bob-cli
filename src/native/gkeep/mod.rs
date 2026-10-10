//! `bob gkeep`: drain the Google Keep inbox into Obsidian tasks.
//!
//! The skeleton pins the CLI surface and shared types; the four
//! subcommand modules start as stubs that later phases implement.

mod adapter;
pub(crate) mod cli;
mod config;
mod doctor;
mod imports;
mod ledger;
mod list;
mod login;
mod migrate;
mod model;
mod plan;
mod pull;
pub(crate) mod render;
mod ui;

use std::{ffi::OsString, iter};

pub(crate) use cli::{DoctorArgs, ListArgs, LoginArgs, MigrateArgs, PullArgs};
// Later phases import the rest directly: `super::cli::ListFormat`,
// `super::config::GkeepConfig`, `super::model::KeepNote`,
// `super::ui::report_error`.

const COMMAND_NAME: &str = "bob gkeep";

/// A typed `bob gkeep` failure with its process exit code.
///
/// Exit codes (all subcommands): 0 is success, including "nothing to
/// pull"; 1 is a runtime failure (auth, network, adapter crash, lock,
/// verify, commit, or an unarchivable note); 2 is a usage or setup
/// error (clap errors, bad config, unknown `--id`, a missing target
/// note, missing `uv`, or no stored token).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GkeepError {
    kind: String,
    message: String,
    hint: Option<String>,
    exit_code: i32,
}

impl GkeepError {
    /// A runtime failure (exit 1).
    pub(crate) fn runtime(kind: &str, message: String) -> Self {
        Self {
            kind: kind.to_string(),
            message,
            hint: None,
            exit_code: 1,
        }
    }

    /// A usage or setup error (exit 2).
    pub(crate) fn setup(kind: &str, message: String) -> Self {
        Self {
            kind: kind.to_string(),
            message,
            hint: None,
            exit_code: 2,
        }
    }

    /// Attach a `hint: …` line to the report.
    pub(crate) fn with_hint(mut self, hint: &str) -> Self {
        self.hint = Some(hint.to_string());
        self
    }

    /// The machine-readable error kind for JSON output.
    pub(crate) fn kind(&self) -> &str {
        &self.kind
    }

    /// The human-readable error message.
    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    /// The optional follow-up hint.
    pub(crate) fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }

    /// The process exit code: 1 for runtime failures, 2 for setup.
    pub(crate) fn exit_code(&self) -> i32 {
        self.exit_code
    }
}

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let command = cli::build_cli();
    let matches = match command.try_get_matches_from(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => {
            let exit_code = error.exit_code();
            if let Err(print_error) = error.print() {
                eprintln!(
                    "{COMMAND_NAME}: failed to print command-line error: \
                     {print_error}"
                );
            }
            return exit_code;
        }
    };

    match matches.subcommand() {
        Some(("doctor", sub_matches)) => {
            doctor::run(&DoctorArgs::from_matches(sub_matches))
        }
        Some(("list", sub_matches)) => {
            list::run(&ListArgs::from_matches(sub_matches))
        }
        Some(("login", sub_matches)) => {
            login::run(&LoginArgs::from_matches(sub_matches))
        }
        Some(("pull", sub_matches)) => {
            pull::run(&PullArgs::from_matches(sub_matches))
        }
        Some(("migrate-markers", sub_matches)) => {
            migrate::run(&MigrateArgs::from_matches(sub_matches))
        }
        Some((name, _)) => {
            eprintln!("{COMMAND_NAME}: unknown subcommand: {name}");
            2
        }
        // No subcommand defaults to `list`; top-level matches carry the
        // same options so `bob gkeep -s vault` works without `list`.
        None => list::run(&ListArgs::from_matches(&matches)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_exit_codes() {
        let error = GkeepError::runtime("network", "down".to_string());
        assert_eq!(error.exit_code(), 1);
        assert_eq!(error.kind(), "network");
        assert_eq!(error.message(), "down");
        assert_eq!(error.hint(), None);

        let error = GkeepError::setup("config", "bad".to_string())
            .with_hint("run `bob gkeep login`");
        assert_eq!(error.exit_code(), 2);
        assert_eq!(error.hint(), Some("run `bob gkeep login`"));
    }
}

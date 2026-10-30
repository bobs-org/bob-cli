//! `bob gcal`: Google Calendar events as daily-note Event Tasks.
//!
//! The read-only `list` command previews the event snapshot and its roots;
//! `sync` writes the rows above today's Pomodoros, idempotently.

mod account;
mod calendars;
mod classify;
pub(crate) mod cli;
pub(crate) mod client;
mod config;
mod doctor;
mod gog;
mod journal;
pub(crate) mod lifecycle;
mod list;
mod login;
mod login_ui;
mod logout;
mod model;
mod plan;
mod render;
mod root;
mod signin;
mod snapshot;
mod stamp;
mod sync;
mod ui;

use std::{ffi::OsString, iter};

pub(crate) use cli::{
    CalendarsArgs, DoctorArgs, ListArgs, LoginArgs, LogoutArgs, SyncArgs,
};

const COMMAND_NAME: &str = "bob gcal";

/// A typed `bob gcal` failure with its process exit code.
///
/// Exit codes (all subcommands): 0 is success; 1 is a runtime failure
/// (gog, network, timeout, token); 2 is a usage or setup error (clap
/// errors, bad config, missing gog, no client, not logged in).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GcalError {
    kind: String,
    message: String,
    hint: Option<String>,
    exit_code: i32,
}

impl GcalError {
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

    pub(crate) fn kind(&self) -> &str {
        &self.kind
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    pub(crate) fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }

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
            if let Err(print_error) = error.print()
                && print_error.kind() != std::io::ErrorKind::BrokenPipe
            {
                eprintln!(
                    "{COMMAND_NAME}: failed to print command-line error: \
                     {print_error}"
                );
            }
            return exit_code;
        }
    };

    match matches.subcommand() {
        Some(("calendars", sub_matches)) => {
            calendars::run(&CalendarsArgs::from_matches(sub_matches))
        }
        Some(("doctor", sub_matches)) => {
            doctor::run(&DoctorArgs::from_matches(sub_matches))
        }
        Some(("list", sub_matches)) => {
            list::run(&ListArgs::from_matches(sub_matches))
        }
        Some(("login", sub_matches)) => {
            login::run(&LoginArgs::from_matches(sub_matches))
        }
        Some(("logout", sub_matches)) => {
            logout::run(&LogoutArgs::from_matches(sub_matches))
        }
        Some(("sync", sub_matches)) => {
            sync::run(&SyncArgs::from_matches(sub_matches))
        }
        Some((name, _)) => {
            eprintln!("{COMMAND_NAME}: unknown subcommand: {name}");
            2
        }
        None => list::run(&ListArgs::from_matches(&matches)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_exit_codes() {
        let error = GcalError::runtime("network", "down".to_string());
        assert_eq!(error.exit_code(), 1);
        assert_eq!(error.kind(), "network");
        assert_eq!(error.message(), "down");
        assert_eq!(error.hint(), None);

        let error = GcalError::setup("config", "bad".to_string())
            .with_hint("run `bob gcal login`");
        assert_eq!(error.exit_code(), 2);
        assert_eq!(error.hint(), Some("run `bob gcal login`"));
    }
}

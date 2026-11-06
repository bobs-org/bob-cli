//! Pure Gmail identity, message data, and body decoding shared by mail consumers.
//!
//! The identity helpers do no I/O; later save/link consumers confirm decoded IDs
//! against Gmail before writing.

// Owned by mail-body (hg.3.1 landing now, hg.3 closing in this turn); allow
// dead code until outer mail-note, gmail-save, and search-peek consumers land.
#[allow(dead_code)]
pub(crate) mod body;
pub(crate) mod cli;
mod config;
mod doctor;
mod gog;
pub(crate) use crate::native::google::{
    account, client, client_setup, login_ui, signin,
};
mod journal;
mod login;
mod logout;
// Owned by mail-note (bob-cli-hg.5); the save and capture phases consume it.
#[allow(dead_code)]
pub(crate) mod note;
#[allow(dead_code)]
pub(crate) mod policy;
// Owned by gmail-save prepare-content (bob-cli-hg.6.1.2); the save
// transaction consumes it.
#[allow(dead_code)]
pub(crate) mod prepare;
#[allow(dead_code)]
pub(crate) mod resolve;
pub(crate) mod ui;
// Owned by gmail-ids (hg.2, closed) and mail-body (hg.3.1 landing now);
// outer mail-note, gmail-save, and search-peek phases consume them.
#[allow(dead_code)]
pub(crate) mod ids;
#[allow(dead_code)]
pub(crate) mod model;

use std::{ffi::OsString, iter};

/// Gmail-facing name for the shared error type.
pub(crate) use crate::native::google::GoogleError as GmailError;

pub(crate) use cli::{DoctorArgs, LoginArgs, LogoutArgs, TopArgs};

const COMMAND_NAME: &str = "bob gmail";

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
        Some(("doctor", sub_matches)) => {
            doctor::run(&DoctorArgs::from_matches(sub_matches))
        }
        Some(("login", sub_matches)) => {
            login::run(&LoginArgs::from_matches(sub_matches))
        }
        Some(("logout", sub_matches)) => {
            logout::run(&LogoutArgs::from_matches(sub_matches))
        }
        Some((name, _)) => {
            eprintln!("{COMMAND_NAME}: unknown subcommand: {name}");
            2
        }
        None => list_saved(&TopArgs::from_matches(&matches)),
    }
}

/// Bare `bob gmail` until gmail-save lands: a friendly offline placeholder.
fn list_saved(args: &TopArgs) -> i32 {
    let config = match config::GmailConfig::resolve() {
        Ok(config) => config,
        Err(error) => {
            return ui::report_error(
                "list",
                &error,
                if args.format.is_json() {
                    "json"
                } else {
                    "human"
                },
            );
        }
    };
    let _ = config;
    if args.format.is_json() {
        println!(
            "{}",
            serde_json::json!({
                "schema_version": 1,
                "ok": true,
                "threads": [],
                "footer": "no saved threads yet",
            })
        );
        ui::flush_stdout();
        return 0;
    }
    println!("No saved threads yet.");
    println!("Save a thread with `bob gmail save`, then list it here.");
    ui::flush_stdout();
    0
}

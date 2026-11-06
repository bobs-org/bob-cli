//! Pure Gmail identity, message data, and body decoding shared by mail consumers.
//!
//! The identity helpers do no I/O; later save/link consumers confirm decoded IDs
//! against Gmail before writing.

// Owned by mail-body (hg.3.1 landing now, hg.3 closing in this turn); allow
// dead code until outer mail-note, gmail-save, and search-peek consumers land.
mod attach;
#[allow(dead_code)]
pub(crate) mod body;
pub(crate) mod capture;
pub(crate) mod cli;
mod compose;
mod config;
mod doctor;
mod gog;
pub(crate) mod list;
pub(crate) mod worker;
pub(crate) use crate::native::google::{
    account, client, client_setup, login_ui, signin,
};
mod journal;
pub(crate) mod link;
mod login;
mod logout;
mod pull;
mod save;
pub(crate) mod search_peek;
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

pub(crate) use cli::{
    AttachArgs, DoctorArgs, LinkArgs, ListArgs, LoginArgs, LogoutArgs, NewArgs,
    PeekArgs, PullArgs, ReplyArgs, SaveArgs, SearchArgs, TopArgs,
};

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
        Some(("attach", sub_matches)) => {
            attach::run(&AttachArgs::from_matches(sub_matches))
        }
        Some(("doctor", sub_matches)) => {
            doctor::run(&DoctorArgs::from_matches(sub_matches))
        }
        Some(("link", sub_matches)) => {
            link::run(&LinkArgs::from_matches(sub_matches))
        }
        Some(("list", sub_matches)) => {
            let args = ListArgs::from_matches(sub_matches);
            list::run(&args)
        }
        Some(("login", sub_matches)) => {
            login::run(&LoginArgs::from_matches(sub_matches))
        }
        Some(("logout", sub_matches)) => {
            logout::run(&LogoutArgs::from_matches(sub_matches))
        }
        Some(("new", sub_matches)) => {
            compose::run_new(&NewArgs::from_matches(sub_matches))
        }
        Some(("peek", sub_matches)) => {
            search_peek::peek(&PeekArgs::from_matches(sub_matches))
        }
        Some(("reply", sub_matches)) => {
            compose::run_reply(&ReplyArgs::from_matches(sub_matches))
        }
        Some(("pull", sub_matches)) => {
            pull::run(&PullArgs::from_matches(sub_matches))
        }
        Some(("save", sub_matches)) => {
            save::run(&SaveArgs::from_matches(sub_matches))
        }
        Some(("search", sub_matches)) => {
            search_peek::search(&SearchArgs::from_matches(sub_matches))
        }
        Some((name, _)) => {
            eprintln!("{COMMAND_NAME}: unknown subcommand: {name}");
            2
        }
        None => {
            let top = TopArgs::from_matches(&matches);
            list::run(&ListArgs {
                all: top.all,
                bob_dir: top.bob_dir,
                format: top.format,
                remote: top.remote,
            })
        }
    }
}

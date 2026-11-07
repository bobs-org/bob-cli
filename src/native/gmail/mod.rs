//! `bob gmail`: saved mail notes, Gmail links, label pull, and draft-first
//! compose.
//!
//! Gmail stays the system of record. Reads go through the read-only gog
//! transport; vault writes are inert, append-only mail notes; sends happen
//! only through a previewed, confirmed Gmail draft.

mod attach;
pub(crate) mod body;
pub(crate) mod capture;
pub(crate) mod cli;
mod compose;
mod config;
mod doctor;
mod gog;
pub(crate) mod ids;
mod journal;
pub(crate) mod link;
pub(crate) mod list;
mod login;
mod logout;
pub(crate) mod model;
pub(crate) mod note;
pub(crate) mod policy;
pub(crate) mod prepare;
mod pull;
pub(crate) mod resolve;
mod route;
mod save;
pub(crate) mod search_peek;
mod sync;
pub(crate) mod turn;
pub(crate) mod ui;
mod vault_walk;
pub(crate) mod worker;
pub(crate) use crate::native::google::{
    account, client, client_setup, login_ui, signin,
};

use std::{ffi::OsString, iter};

/// Gmail-facing name for the shared error type.
pub(crate) use crate::native::google::GoogleError as GmailError;

pub(crate) use cli::{
    AttachArgs, DoctorArgs, DraftArgs, LinkArgs, ListArgs, LoginArgs,
    LogoutArgs, NewArgs, PeekArgs, PullArgs, ReplyArgs, SaveArgs, SearchArgs,
    SendArgs, TopArgs,
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
        Some(("draft", sub_matches)) => {
            compose::run_draft(&DraftArgs::from_matches(sub_matches))
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
        Some(("send", sub_matches)) => {
            compose::run_send(&SendArgs::from_matches(sub_matches))
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

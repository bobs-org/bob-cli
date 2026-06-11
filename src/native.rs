use std::ffi::OsString;

mod collect_done;
mod dataview;
mod env;
mod highlights_ref;
mod nightly;
mod notify;
mod ob;
mod pomodoro;
mod projects;
mod sync;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeCommand {
    BulkGitCommit,
    Dataview,
    Highlights,
    MoveDoneTasks,
    Nightly,
    Notify,
    Pomodoro,
    Projects,
    TmuxPomodoro,
}

pub(crate) fn command_for_script(
    script_command: &str,
) -> Option<NativeCommand> {
    match script_command {
        "bob_pomodoro" => Some(NativeCommand::Pomodoro),
        "bob_notify" => Some(NativeCommand::Notify),
        "bob_sync" => Some(NativeCommand::BulkGitCommit),
        "tmux_bob_pomodoro" => Some(NativeCommand::TmuxPomodoro),
        _ => None,
    }
}

pub(crate) fn run(command: NativeCommand, args: Vec<OsString>) -> i32 {
    match command {
        NativeCommand::BulkGitCommit => sync::run(args),
        NativeCommand::Dataview => dataview::run(args),
        NativeCommand::Highlights => highlights_ref::run(args),
        NativeCommand::MoveDoneTasks => collect_done::run(args),
        NativeCommand::Nightly => nightly::run(args),
        NativeCommand::Notify => notify::run(args),
        NativeCommand::Pomodoro => pomodoro::run(args),
        NativeCommand::Projects => projects::run(args),
        NativeCommand::TmuxPomodoro => pomodoro::run_tmux(args),
    }
}

pub(crate) fn pomodoro_status() -> Result<Option<String>, pomodoro::Error> {
    pomodoro::status_from_env()
}

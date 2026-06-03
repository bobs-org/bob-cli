use std::ffi::OsString;

mod collect_done;
mod cronjob;
mod env;
mod highlights_ref;
mod notify;
mod ob;
mod pomodoro;
mod sync;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeCommand {
    CollectDone,
    Cronjob,
    HighlightsRef,
    Pomodoro,
    Notify,
    Sync,
    TmuxPomodoro,
}

pub(crate) fn command_for_script(
    script_command: &str,
) -> Option<NativeCommand> {
    match script_command {
        "bob_pomodoro" => Some(NativeCommand::Pomodoro),
        "bob_notify" => Some(NativeCommand::Notify),
        "bob_sync" => Some(NativeCommand::Sync),
        "tmux_bob_pomodoro" => Some(NativeCommand::TmuxPomodoro),
        _ => None,
    }
}

pub(crate) fn run(command: NativeCommand, args: Vec<OsString>) -> i32 {
    match command {
        NativeCommand::CollectDone => collect_done::run(args),
        NativeCommand::Cronjob => cronjob::run(args),
        NativeCommand::HighlightsRef => highlights_ref::run(args),
        NativeCommand::Pomodoro => pomodoro::run(args),
        NativeCommand::Notify => notify::run(args),
        NativeCommand::Sync => sync::run(args),
        NativeCommand::TmuxPomodoro => pomodoro::run_tmux(args),
    }
}

pub(crate) fn pomodoro_status() -> Result<Option<String>, pomodoro::Error> {
    pomodoro::status_from_env()
}

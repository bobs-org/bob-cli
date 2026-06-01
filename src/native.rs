use std::ffi::OsString;

mod env;
mod notify;
mod pomodoro;
mod runtimes;
mod sync;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeCommand {
    Pomodoro,
    PomodoroRuntimes,
    Notify,
    Sync,
    TmuxPomodoro,
}

pub(crate) fn command_for_script(
    script_command: &str,
) -> Option<NativeCommand> {
    match script_command {
        "bob_pomodoro" => Some(NativeCommand::Pomodoro),
        "bob_pomodoro_runtimes" => Some(NativeCommand::PomodoroRuntimes),
        "bob_notify" => Some(NativeCommand::Notify),
        "bob_sync" => Some(NativeCommand::Sync),
        "tmux_bob_pomodoro" => Some(NativeCommand::TmuxPomodoro),
        _ => None,
    }
}

pub(crate) fn run(command: NativeCommand, args: Vec<OsString>) -> i32 {
    match command {
        NativeCommand::Pomodoro => pomodoro::run(args),
        NativeCommand::PomodoroRuntimes => runtimes::run(args),
        NativeCommand::Notify => notify::run(args),
        NativeCommand::Sync => sync::run(args),
        NativeCommand::TmuxPomodoro => pomodoro::run_tmux(args),
    }
}

pub(crate) fn pomodoro_status() -> Result<Option<String>, pomodoro::Error> {
    pomodoro::status_from_env()
}

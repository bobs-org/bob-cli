use std::ffi::{OsStr, OsString};

const ALWAYS_EXCLUDED_NOTE_DIRECTORY_NAMES: &[&str] =
    &[".git", ".obsidian", "_generated", "_templates"];

fn is_always_excluded_note_directory_name(name: &OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        ALWAYS_EXCLUDED_NOTE_DIRECTORY_NAMES.contains(&name)
    })
}

mod capture;
mod capture_clip;
mod capture_sections;
mod capture_targets;
mod collect_done;
mod dataview;
mod env;
mod highlights_ref;
mod mark_next;
mod markdown;
mod nightly;
mod notify;
mod ob;
mod plugins;
mod pomodoro;
mod projects;
mod style;
mod sync;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeCommand {
    BulkGitCommit,
    Capture,
    CaptureSections,
    CaptureTargets,
    Query,
    Highlights,
    MarkNextTasks,
    MoveDoneTasks,
    Nightly,
    Notify,
    Plugins,
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
        NativeCommand::Capture => capture::run(args),
        NativeCommand::CaptureSections => capture_sections::run(args),
        NativeCommand::CaptureTargets => capture_targets::run(args),
        NativeCommand::Query => dataview::run(args),
        NativeCommand::Highlights => highlights_ref::run(args),
        NativeCommand::MarkNextTasks => mark_next::run(args),
        NativeCommand::MoveDoneTasks => collect_done::run(args),
        NativeCommand::Nightly => nightly::run(args),
        NativeCommand::Notify => notify::run(args),
        NativeCommand::Plugins => plugins::run(args),
        NativeCommand::Pomodoro => pomodoro::run(args),
        NativeCommand::Projects => projects::run(args),
        NativeCommand::TmuxPomodoro => pomodoro::run_tmux(args),
    }
}

pub(crate) fn pomodoro_status() -> Result<Option<String>, pomodoro::Error> {
    pomodoro::status_from_env()
}

use std::ffi::{OsStr, OsString};

const ALWAYS_EXCLUDED_NOTE_DIRECTORY_NAMES: &[&str] = &[
    ".git",
    ".obsidian",
    "_conflicts",
    "_generated",
    "_templates",
];

pub(crate) fn is_always_excluded_note_directory_name(name: &OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        ALWAYS_EXCLUDED_NOTE_DIRECTORY_NAMES.contains(&name)
    })
}

mod capture;
mod capture_clip;
mod capture_complete;
mod capture_language;
mod capture_links;
mod capture_parse;
mod capture_rewrite;
mod capture_schedule_log;
mod capture_sections;
mod capture_targets;
mod capture_task_id;
mod capture_task_sections;
mod capture_tasks;
mod collect_done;
mod config;
mod dataview;
mod env;
mod highlights_ref;
mod markdown;
mod nightly;
mod note_tasks;
mod notify;
mod ob;
mod plugins;
mod pomodoro;
mod projects;
mod style;
mod task_status_hooks;
mod vault_sync;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeCommand {
    Capture,
    CaptureComplete,
    CaptureParse,
    CaptureRewrite,
    CaptureSections,
    CaptureTargets,
    CaptureTaskId,
    CaptureTaskSections,
    CaptureTasks,
    Query,
    Highlights,
    MoveDoneTasks,
    Nightly,
    Notify,
    Plugins,
    Pomodoro,
    Projects,
    TaskStatusHooks,
    TmuxPomodoro,
    VaultSync,
}

pub(crate) fn command_for_script(
    script_command: &str,
) -> Option<NativeCommand> {
    match script_command {
        "bob_pomodoro" => Some(NativeCommand::Pomodoro),
        "bob_notify" => Some(NativeCommand::Notify),
        "tmux_bob_pomodoro" => Some(NativeCommand::TmuxPomodoro),
        _ => None,
    }
}

pub(crate) fn run(command: NativeCommand, args: Vec<OsString>) -> i32 {
    match command {
        NativeCommand::Capture => capture::run(args),
        NativeCommand::CaptureComplete => capture_complete::run(args),
        NativeCommand::CaptureParse => capture_parse::run(args),
        NativeCommand::CaptureRewrite => capture_rewrite::run(args),
        NativeCommand::CaptureSections => capture_sections::run(args),
        NativeCommand::CaptureTargets => capture_targets::run(args),
        NativeCommand::CaptureTaskId => capture_task_id::run(args),
        NativeCommand::CaptureTaskSections => capture_task_sections::run(args),
        NativeCommand::CaptureTasks => capture_tasks::run(args),
        NativeCommand::Query => dataview::run(args),
        NativeCommand::Highlights => highlights_ref::run(args),
        NativeCommand::MoveDoneTasks => collect_done::run(args),
        NativeCommand::Nightly => nightly::run(args),
        NativeCommand::Notify => notify::run(args),
        NativeCommand::Plugins => plugins::run(args),
        NativeCommand::Pomodoro => pomodoro::run(args),
        NativeCommand::Projects => projects::run(args),
        NativeCommand::TaskStatusHooks => task_status_hooks::run(args),
        NativeCommand::TmuxPomodoro => pomodoro::run_tmux(args),
        NativeCommand::VaultSync => vault_sync::run(args),
    }
}

pub(crate) fn pomodoro_status() -> Result<Option<String>, pomodoro::Error> {
    pomodoro::status_from_env()
}

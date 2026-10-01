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
mod capture_active_tasks;
mod capture_block_ids;
mod capture_clip;
mod capture_complete;
mod capture_language;
mod capture_link_tasks;
mod capture_links;
mod capture_parse;
mod capture_pomodoro_close;
mod capture_pomodoro_name;
mod capture_pomodoro_start;
mod capture_pomodoros;
mod capture_project_note;
mod capture_rewrite;
mod capture_schedule_log;
mod capture_sections;
mod capture_targets;
mod capture_task_id;
mod capture_task_sections;
mod capture_task_toggle;
mod capture_tasks;
mod capture_work_log;
mod collect_done;
mod config;
mod dataview;
mod env;
mod freshness;
mod gkeep;
mod highlights_ref;
mod markdown;
mod nightly;
mod note_ready;
mod note_tasks;
mod notify;
mod ob;
mod plan_budget;
mod plugins;
mod pomodoro;
mod projects;
mod randomize;
mod randomize_plan;
mod style;
mod task_fields;
mod task_status_groups;
mod task_status_hooks;
mod task_status_hooks_write;
mod vault_links;
mod vault_sync;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeCommand {
    Capture,
    CaptureComplete,
    CaptureParse,
    CapturePomodoroName,
    CapturePomodoros,
    CaptureRewrite,
    CaptureSections,
    CaptureTargets,
    CaptureTaskId,
    CaptureTaskSections,
    CaptureTasks,
    Freshness,
    Gkeep,
    Query,
    Highlights,
    MoveDoneTasks,
    Nightly,
    NoteReady,
    Notify,
    Plan,
    Plugins,
    Pomodoro,
    Projects,
    Randomize,
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
        NativeCommand::CapturePomodoroName => capture_pomodoro_name::run(args),
        NativeCommand::CapturePomodoros => capture_pomodoros::run(args),
        NativeCommand::CaptureRewrite => capture_rewrite::run(args),
        NativeCommand::CaptureSections => capture_sections::run(args),
        NativeCommand::CaptureTargets => capture_targets::run(args),
        NativeCommand::CaptureTaskId => capture_task_id::run(args),
        NativeCommand::CaptureTaskSections => capture_task_sections::run(args),
        NativeCommand::CaptureTasks => capture_tasks::run(args),
        NativeCommand::Freshness => freshness::cli::run(args),
        NativeCommand::Gkeep => gkeep::run(args),
        NativeCommand::Query => dataview::run(args),
        NativeCommand::Highlights => highlights_ref::run(args),
        NativeCommand::MoveDoneTasks => collect_done::run(args),
        NativeCommand::Nightly => nightly::run(args),
        NativeCommand::NoteReady => note_ready::cli::run(args),
        NativeCommand::Notify => notify::run(args),
        NativeCommand::Plan => plan_budget::cli::run(args),
        NativeCommand::Plugins => plugins::run(args),
        NativeCommand::Pomodoro => pomodoro::run(args),
        NativeCommand::Projects => projects::run(args),
        NativeCommand::Randomize => randomize::run(args),
        NativeCommand::TaskStatusHooks => task_status_hooks::run(args),
        NativeCommand::TmuxPomodoro => pomodoro::run_tmux(args),
        NativeCommand::VaultSync => vault_sync::run(args),
    }
}

pub(crate) fn pomodoro_status() -> Result<Option<String>, pomodoro::Error> {
    pomodoro::status_from_env()
}

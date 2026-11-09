use std::ffi::{OsStr, OsString};

const ALWAYS_EXCLUDED_NOTE_DIRECTORY_NAMES: &[&str] = &[
    ".bob",
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

mod articles;
pub(crate) mod attachments;
mod capture;
mod capture_active_tasks;
mod capture_block_ids;
mod capture_clip;
mod capture_completable_tasks;
mod capture_complete;
mod capture_dependency_tasks;
mod capture_drawers;
mod capture_flag_guard;
mod capture_language;
mod capture_link_tasks;
mod capture_links;
mod capture_parse;
mod capture_pomodoro_close;
mod capture_pomodoro_name;
mod capture_pomodoro_start;
mod capture_pomodoros;
mod capture_pomodoros_agenda;
mod capture_project_note;
pub(crate) mod capture_record;
mod capture_related;
mod capture_rewrite;
mod capture_schedule_log;
mod capture_sections;
mod capture_targets;
mod capture_task_id;
mod capture_task_toggle;
mod capture_task_wikilinks;
mod capture_tasks;
mod capture_templates;
mod capture_work_log;
pub(crate) mod catalog;
mod clipboard_status;
mod clock_tags;
mod collect_done;
pub(crate) mod completion;
mod config;
mod dataview;
pub(crate) mod env;
mod event_task;
// The executor phase (bob-cli-at.3) will connect this primitive to PDF writes.
#[allow(dead_code)]
pub(crate) mod file_coordination;
mod freshness;
mod gcal;
// Consumer: the Gmail renderer phases bob-cli-hg.3.1.2–.4 consume this pure model.
mod gkeep;
pub(crate) mod gmail;
pub(crate) mod google;
mod highlights_ref;
mod ledger_place;
mod markdown;
mod nightly;
mod note_ready;
mod note_tasks;
pub(crate) mod notebooks;
mod notify;
mod ob;
pub(crate) mod parent_notes;
pub(crate) mod periods;
mod plan_budget;
mod plugins;
mod pomodoro;
mod pomodoro_homes;
mod pomodoro_themes;
mod pref_journal;
mod project_dependencies;
mod projects;
mod randomize;
mod randomize_plan;
pub(crate) mod records;
mod ref_jobs;
pub(crate) mod ref_library;
pub(crate) mod ref_tasks;
mod relation_labels;
mod spinner;
mod stats;
pub(crate) mod style;
mod subprocess_runner;
// Shared cancel-label policy and pure effect engine.
// Later epic phases connect these primitives to explicit relation gestures.
#[cfg_attr(not(test), allow(dead_code))]
mod task_cancel;
mod task_children;
mod task_command_shell;
mod task_complete;
pub(crate) mod task_dependencies;
mod task_fields;
mod task_locator;
mod task_merge;
mod task_move;
pub(crate) mod task_relations;
mod task_root;
mod task_split;
mod task_status_groups;
mod task_status_hooks;
mod task_status_hooks_write;
mod task_text_escape;
mod task_theme;
mod templater;
#[cfg(test)]
pub(crate) mod test_stub;
pub(crate) mod url_routing;
mod vault_clobber;
pub(crate) mod vault_git;
mod vault_links;
mod vault_merge_union;
mod vault_rewrite;
mod vault_sync;
mod vault_task_loss;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeCommand {
    Articles,
    AttachmentsImport,
    AttachmentsOrigin,
    Capture,
    CaptureComplete,
    CaptureParse,
    CapturePomodoroName,
    CapturePomodoros,
    CaptureRelated,
    CaptureRewrite,
    CaptureSections,
    CaptureTargets,
    CaptureTaskId,
    CaptureDrawers,
    CaptureTasks,
    CaptureTemplatesClear,
    CaptureTemplatesDelete,
    CaptureTemplatesList,
    CaptureTemplatesSave,
    CaptureTemplatesShow,
    ClipboardStatus,
    Completion,
    Freshness,
    Gcal,
    Gkeep,
    Gmail,
    Query,
    Highlights,
    MoveDoneTasks,
    TaskMerge,
    TaskMove,
    TaskSplit,
    Nightly,
    NotebooksCheck,
    NotebooksCreate,
    NotebooksList,
    NotebooksSync,
    NoteReady,
    Notify,
    PeriodsBrief,
    PeriodsDraft,
    PeriodsEnsure,
    PeriodsList,
    PeriodsMigrate,
    PeriodsShow,
    Plan,
    Plugins,
    Pomodoro,
    Projects,
    Randomize,
    Stats,
    StatsMetrics,
    StatsShow,
    TaskStatusHooks,
    ThemesColor,
    ThemesHide,
    ThemesList,
    ThemesMigrate,
    ThemesRename,
    ThemesSplit,
    ThemesSuggest,
    ThemesUnhide,
    RelationsHide,
    RelationsList,
    RelationsRename,
    RelationsUnhide,
    RecordsCheck,
    RecordsInit,
    RecordsList,
    RecordsNew,
    RecordsShow,
    RecordsUpdate,
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
        NativeCommand::Articles => articles::run(args),
        NativeCommand::AttachmentsImport => attachments::run_import(args),
        NativeCommand::AttachmentsOrigin => attachments::run_origin(args),
        NativeCommand::Capture => capture::run(args),
        NativeCommand::CaptureComplete => capture_complete::run(args),
        NativeCommand::CaptureParse => capture_parse::run(args),
        NativeCommand::CapturePomodoroName => capture_pomodoro_name::run(args),
        NativeCommand::CapturePomodoros => capture_pomodoros::run(args),
        NativeCommand::CaptureRelated => capture_related::run(args),
        NativeCommand::CaptureRewrite => capture_rewrite::run(args),
        NativeCommand::CaptureSections => capture_sections::run(args),
        NativeCommand::CaptureTargets => capture_targets::run(args),
        NativeCommand::CaptureTaskId => capture_task_id::run(args),
        NativeCommand::CaptureDrawers => capture_drawers::run(args),
        NativeCommand::CaptureTasks => capture_tasks::run(args),
        NativeCommand::CaptureTemplatesClear => {
            capture_templates::command::run_clear(args)
        }
        NativeCommand::CaptureTemplatesDelete => {
            capture_templates::command::run_delete(args)
        }
        NativeCommand::CaptureTemplatesList => {
            capture_templates::command::run_list(args)
        }
        NativeCommand::CaptureTemplatesSave => {
            capture_templates::command::run_save(args)
        }
        NativeCommand::CaptureTemplatesShow => {
            capture_templates::command::run_show(args)
        }
        NativeCommand::ClipboardStatus => clipboard_status::run(args),
        NativeCommand::Completion => completion::run(args),
        NativeCommand::Freshness => freshness::cli::run(args),
        NativeCommand::Gcal => gcal::run(args),
        NativeCommand::Gkeep => gkeep::run(args),
        NativeCommand::Gmail => gmail::run(args),
        NativeCommand::Query => dataview::run(args),
        NativeCommand::Highlights => highlights_ref::run(args),
        NativeCommand::MoveDoneTasks => collect_done::run(args),
        NativeCommand::TaskMerge => task_merge::run(args),
        NativeCommand::TaskMove => task_move::run(args),
        NativeCommand::TaskSplit => task_split::run(args),
        NativeCommand::Nightly => nightly::run(args),
        NativeCommand::NotebooksCheck => notebooks::cli::run_check(args),
        NativeCommand::NotebooksCreate => notebooks::cli::run_create(args),
        NativeCommand::NotebooksList => notebooks::cli::run_list(args),
        NativeCommand::NotebooksSync => notebooks::cli::run_sync(args),
        NativeCommand::NoteReady => note_ready::cli::run(args),
        NativeCommand::Notify => notify::run(args),
        NativeCommand::PeriodsBrief => periods::brief::run(args),
        NativeCommand::PeriodsDraft => periods::draft::run(args),
        NativeCommand::PeriodsEnsure => periods::ensure::run(args),
        NativeCommand::PeriodsList => periods::list::run(args),
        NativeCommand::PeriodsMigrate => periods::migrate::run(args),
        NativeCommand::PeriodsShow => periods::report::run(args),
        NativeCommand::Plan => plan_budget::cli::run(args),
        NativeCommand::Plugins => plugins::run(args),
        NativeCommand::Pomodoro => pomodoro::run(args),
        NativeCommand::Projects => projects::run(args),
        NativeCommand::Randomize => randomize::run(args),
        NativeCommand::Stats => stats::cli::run_overview(args),
        NativeCommand::StatsMetrics => stats::cli::run_metrics(args),
        NativeCommand::StatsShow => stats::cli::run_show(args),
        NativeCommand::TaskStatusHooks => task_status_hooks::run(args),
        NativeCommand::RelationsHide => relation_labels::run_hide(args),
        NativeCommand::RelationsList => relation_labels::run_list(args),
        NativeCommand::RelationsRename => relation_labels::run_rename(args),
        NativeCommand::RelationsUnhide => relation_labels::run_unhide(args),
        NativeCommand::RecordsCheck => records::cli::run_check(args),
        NativeCommand::RecordsInit => records::cli::run_init(args),
        NativeCommand::RecordsList => records::cli::run_list(args),
        NativeCommand::RecordsNew => records::cli::run_new(args),
        NativeCommand::RecordsShow => records::cli::run_show(args),
        NativeCommand::RecordsUpdate => records::cli::run_update(args),
        NativeCommand::ThemesColor => pomodoro_themes::run_color(args),
        NativeCommand::ThemesHide => pomodoro_themes::run_hide(args),
        NativeCommand::ThemesList => pomodoro_themes::run_list(args),
        NativeCommand::ThemesMigrate => pomodoro_themes::run_migrate(args),
        NativeCommand::ThemesRename => pomodoro_themes::run_rename(args),
        NativeCommand::ThemesSplit => pomodoro_themes::run_split(args),
        NativeCommand::ThemesSuggest => pomodoro_themes::suggest_cli::run(args),
        NativeCommand::ThemesUnhide => pomodoro_themes::run_unhide(args),
        NativeCommand::TmuxPomodoro => pomodoro::run_tmux(args),
        NativeCommand::VaultSync => vault_sync::run(args),
    }
}

pub(crate) fn validate_day_timezone() -> Result<(), config::ConfigError> {
    config::load_day_timezone(&config::config_path()).map(|_| ())
}

pub(crate) fn day_timezone(
) -> Result<Option<chrono_tz::Tz>, config::ConfigError> {
    config::load_day_timezone(&config::config_path())
}

pub(crate) fn pomodoro_status() -> Result<Option<String>, pomodoro::Error> {
    pomodoro::status_from_env()
}

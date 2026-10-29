//! Read-only vault providers for vault-aware completion slots.
//!
//! Every provider reuses the existing in-process scanners against the
//! [`Context`](super::context::Context) vault root, so completion can
//! never disagree with the command it completes. Providers only read:
//! no vault writes, no `git`, no network, no state files. The
//! read-only enforcement test in `tests/cli/completion/vault.rs`
//! proves it on a write-locked vault.
//!
//! A dependent slot with its prerequisite missing answers
//! `!message pass --route first` (or `pass --task first`) instead of
//! guessing.

use std::{ffi::OsStr, fs, path::Path};

use super::{context::Context, kinds::Kind, protocol};
use crate::native::{
    capture, capture_pomodoros, capture_targets, capture_task_sections, config,
    note_tasks, pomodoro,
};

/// Shown when a route-dependent slot has no `--route` yet.
pub(crate) const MISSING_ROUTE_MESSAGE: &str = "pass --route first";
/// Shown when `--task-section` has a route but no `--task` yet.
pub(crate) const MISSING_TASK_MESSAGE: &str = "pass --task first";

/// Answer one vault slot. Returns `None` when `kind` is not
/// vault-aware, so the presenter keeps its static decision.
pub(crate) fn vault_lines(
    kind: Kind,
    context: &Context,
) -> Option<Vec<String>> {
    match kind {
        Kind::Route => Some(routes(&context.bob_dir)),
        Kind::Section => Some(sections(context)),
        Kind::Task => Some(tasks(context)),
        Kind::TaskSection => Some(task_sections(context)),
        Kind::PomodoroRef => Some(pomodoros(context)),
        Kind::Plugin => Some(plugins(context)),
        Kind::Level => Some(levels()),
        Kind::Metric => Some(metrics()),
        Kind::RelationLabel => Some(relation_labels(context)),
        Kind::ThemeName => Some(theme_names(context)),
        Kind::VaultNote => Some(vault_notes(context)),
        Kind::ArticleNote => Some(article_notes(context)),
        Kind::Choices | Kind::Dirs | Kind::Files(_) | Kind::FreeText => None,
    }
}

/// Remembered Pomodoro themes, including hidden names so hide, unhide,
/// color, and rename can all operate on the existing catalog. This uses
/// the completion-only catalog load and performs no vault writes.
fn theme_names(context: &Context) -> Vec<String> {
    let today = crate::native::env::current_datetime().date();
    let Ok(catalog) =
        crate::native::pomodoro_themes::Catalog::load_for_completion(
            &context.bob_dir,
            today,
            Default::default(),
            None,
        )
    else {
        return Vec::new();
    };
    let mut names: Vec<String> = catalog
        .records()
        .filter_map(|record| {
            protocol::candidate_line(
                OsStr::new(&record.name),
                "remembered theme",
                "themes",
                false,
            )
        })
        .collect();
    if let Some(entries) = catalog.configured_themes() {
        let known: std::collections::BTreeSet<&str> = catalog
            .records()
            .map(|record| record.key.as_str())
            .collect();
        for entry in entries {
            if known.contains(entry.key.as_str()) {
                continue;
            }
            if let Some(line) = protocol::candidate_line(
                OsStr::new(&entry.name),
                "configured theme",
                "themes",
                false,
            ) {
                names.push(line);
            }
        }
    }
    names
}

/// Relation labels, including hidden labels so rename can recover a label
/// that is no longer suggested by the default list. Configured cancel
/// labels note their consequence (`N links · cancels source` / `N links ·
/// completes source`).
fn relation_labels(context: &Context) -> Vec<String> {
    let Ok(catalog) =
        crate::native::relation_labels::Catalog::load(&context.bob_dir)
    else {
        return Vec::new();
    };
    let policy = crate::native::config::load_relations_config(
        &crate::native::config::config_path(),
    )
    .unwrap_or_default();
    catalog
        .records()
        .filter_map(|record| {
            let hidden = catalog.is_hidden(&record.key);
            let mut description = format!(
                "{} link{}",
                record.count,
                if record.count == 1 { "" } else { "s" },
            );
            if policy.is_cancel_label(&record.key) {
                description.push_str(" · cancels source");
            }
            if policy.is_complete_label(&record.key) {
                description.push_str(" · completes source");
            }
            if hidden {
                description.push_str(" · hidden");
            }
            protocol::candidate_line(
                OsStr::new(&record.name),
                &description,
                "relation labels",
                false,
            )
        })
        .collect()
}

/// Routes in capture-targets scan order: the default inbox, then
/// areas, then projects. Groups `inbox` / `areas` / `projects`.
fn routes(bob_dir: &Path) -> Vec<String> {
    let report = capture_targets::scan_capture_targets(bob_dir);
    let mut lines = Vec::with_capacity(report.targets.len());
    for target in &report.targets {
        let (group, description) = match target.kind {
            capture_targets::CaptureTargetKind::Inbox => {
                ("inbox", "inbox · default capture target".to_string())
            }
            capture_targets::CaptureTargetKind::Area => {
                ("areas", "area".to_string())
            }
            capture_targets::CaptureTargetKind::Project => (
                "projects",
                target
                    .status
                    .as_deref()
                    .map(|status| format!("project · {status}"))
                    .unwrap_or_else(|| "project".to_string()),
            ),
        };
        if let Some(line) = protocol::candidate_line(
            OsStr::new(&target.route),
            &description,
            group,
            false,
        ) {
            lines.push(line);
        }
    }
    lines
}

/// Non-Tasks sections of the routed note, in document order under
/// `sections in <route>`. A missing note is not an error: it offers
/// nothing so picker callers skip the chooser.
fn sections(context: &Context) -> Vec<String> {
    let Some(route) = context.route.as_deref() else {
        return vec![protocol::message_line(MISSING_ROUTE_MESSAGE)];
    };
    let contents = match fs::read_to_string(
        context.bob_dir.join(capture::route_label(route)),
    ) {
        Ok(contents) => contents,
        Err(_) => return Vec::new(),
    };
    let group = format!("sections in {route}");
    capture::non_tasks_section_headings(&contents)
        .iter()
        .filter_map(|heading| {
            protocol::candidate_line(
                OsStr::new(&heading.title),
                &format!("H{}", heading.level),
                &group,
                false,
            )
        })
        .collect()
}

/// Open tasks of the routed note, in scanner order under
/// `tasks in <route>`. The value is the block ID exactly as the
/// flag expects it; tasks without one are skipped because the flag
/// cannot name them.
fn tasks(context: &Context) -> Vec<String> {
    let Some(route) = context.route.as_deref() else {
        return vec![protocol::message_line(MISSING_ROUTE_MESSAGE)];
    };
    let group = format!("tasks in {route}");
    open_tasks(context, route)
        .into_iter()
        .filter_map(|task| {
            protocol::candidate_line(
                OsStr::new(task.block_id.as_deref()?),
                &task.description,
                &group,
                false,
            )
        })
        .collect()
}

fn open_tasks(context: &Context, route: &str) -> Vec<note_tasks::NoteTask> {
    let contents = match fs::read_to_string(
        context.bob_dir.join(capture::route_label(route)),
    ) {
        Ok(contents) => contents,
        Err(_) => return Vec::new(),
    };
    let settings = note_tasks::read_settings(&context.bob_dir);
    note_tasks::scan(&contents, &settings)
        .open_tasks()
        .cloned()
        .collect()
}

/// ALL-CAPS child sections of the `--task` parent, in document order
/// under `task sections`. The value is the exact TITLE the flag
/// expects.
fn task_sections(context: &Context) -> Vec<String> {
    let Some(route) = context.route.as_deref() else {
        return vec![protocol::message_line(MISSING_ROUTE_MESSAGE)];
    };
    let Some(task) = context.task.as_deref() else {
        return vec![protocol::message_line(MISSING_TASK_MESSAGE)];
    };
    let contents = match fs::read_to_string(
        context.bob_dir.join(capture::route_label(route)),
    ) {
        Ok(contents) => contents,
        Err(_) => return Vec::new(),
    };
    let settings = note_tasks::read_settings(&context.bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    let note_tasks::BlockIdLookup::Found(parent) = scan.by_block_id(task)
    else {
        return Vec::new();
    };
    capture_task_sections::task_sections(&contents, parent)
        .iter()
        .filter_map(|section| {
            let description = match section.child_count {
                0 => String::new(),
                1 => "1 item".to_string(),
                count => format!("{count} items"),
            };
            protocol::candidate_line(
                OsStr::new(&section.title),
                &description,
                "task sections",
                false,
            )
        })
        .collect()
}

/// Open Pomodoros from today's daily note under `open Pomodoros`.
/// The value is the stale-safe ref the flag expects; the description
/// pairs the time range with the name.
fn pomodoros(context: &Context) -> Vec<String> {
    let day_file = pomodoro::day_file_for(&context.bob_dir);
    let contents = match fs::read_to_string(&day_file) {
        Ok(contents) => contents,
        Err(_) => return Vec::new(),
    };
    capture_pomodoros::scan(&contents)
        .entries
        .iter()
        .filter(|entry| entry.state == capture_pomodoros::PomodoroState::Open)
        .filter_map(|entry| {
            let description = match (&entry.time_range, &entry.name) {
                (Some(time), Some(name)) => format!("{time} · {name}"),
                (Some(time), None) => format!("{time} · open"),
                (None, Some(name)) => name.clone(),
                (None, None) => "open".to_string(),
            };
            protocol::candidate_line(
                OsStr::new(&entry.pomodoro_ref.to_string()),
                &description,
                "open Pomodoros",
                false,
            )
        })
        .collect()
}

/// Plugin IDs from the repo checkout: directory names under
/// `<repo>/plugins/*/`, sorted. Never git: this is a directory read.
fn plugins(context: &Context) -> Vec<String> {
    let Some(repo) = context.repo.as_deref() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(repo.join("plugins")) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_dir())
            && let Some(name) = entry.file_name().to_str().map(str::to_string)
        {
            names.push(name);
        }
    }
    names.sort();
    names
        .iter()
        .filter_map(|name| {
            protocol::candidate_line(OsStr::new(name), "", "plugins", false)
        })
        .collect()
}

/// Live metric ids from the stats registry. Hidden ids stay completable
/// so `bob stats show` can name them explicitly.
fn metrics() -> Vec<String> {
    let config = match config::load_stats_config(&config::config_path()) {
        Ok(config) => config,
        Err(_) => return Vec::new(),
    };
    let registry = crate::native::stats::build_registry(&config);
    registry
        .defs
        .iter()
        .filter_map(|def| {
            let group = if registry.is_hidden(&def.id) {
                "hidden"
            } else {
                "metrics"
            };
            protocol::candidate_line(
                OsStr::new(&def.id),
                def.kind_name(),
                group,
                false,
            )
        })
        .collect()
}

/// Configured `randomize` priority labels in config order; the
/// description is the roll window. A missing config offers nothing.
fn levels() -> Vec<String> {
    let property = match config::load_priority_property(&config::config_path())
    {
        Ok(property) => property,
        Err(_) => return Vec::new(),
    };
    property
        .levels()
        .iter()
        .filter_map(|level| {
            protocol::candidate_line(
                OsStr::new(level.label()),
                &format!("{}–{} days", level.min_days(), level.max_days()),
                "label",
                false,
            )
        })
        .collect()
}

/// Vault notes complete as paths relative to the vault root.
fn vault_notes(context: &Context) -> Vec<String> {
    vec![protocol::files_in_line(
        &context.bob_dir.display().to_string(),
        "*.md",
    )]
}

/// Article notes complete by their vault-relative stem and expose the
/// current section count in the candidate description.
fn article_notes(context: &Context) -> Vec<String> {
    crate::native::articles::completion_candidates(&context.bob_dir)
        .iter()
        .filter_map(|(path, description)| {
            protocol::candidate_line(
                OsStr::new(path),
                description,
                "articles",
                false,
            )
        })
        .collect()
}

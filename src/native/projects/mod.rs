use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::OsString,
    fs, io, iter,
    path::{Path, PathBuf},
};

use chrono::NaiveDate;
use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};

use super::{
    env as bob_env, is_always_excluded_note_directory_name,
    style::{display_width, pad_right, Styler},
};

const COMMAND_NAME: &str = "bob projects";
const PLACEHOLDER_CRITERIA: &str =
    "<short_project_completion_criteria_goes_here>";
const PROJECT_TASK_SHAPE: &str =
    "- [ ] #task #prj <completion criteria> #hide ^prj";
const HIDE_TAG: &str = "#hide";
const PROJECT_TASK_TAG: &str = "#prj";
const PROJECT_BLOCKED_TAG: &str = "#prj_is_blocked";
pub(crate) const SUBPROJECTS_MARKER_PREFIX: &str = "🧩 **Sub-projects:**";
const SUBPROJECTS_SEPARATOR: &str = "•";
pub(crate) const SUBPROJECT_FUTURE_SCHEDULE_MARKER: &str = "🗓️";
pub(crate) const SUBPROJECT_DONE_MARKER: &str = "✅";
pub(crate) const SUBPROJECT_CANCELED_MARKER: &str = "❌";

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    match matches.subcommand() {
        Some(("list", sub_matches)) => run_list(sub_matches),
        Some(("sync", sub_matches)) => run_sync(sub_matches),
        Some((name, _)) => {
            eprintln!("{COMMAND_NAME}: unknown subcommand: {name}");
            2
        }
        None => 2,
    }
}

fn print_clap_error(error: clap::Error) -> i32 {
    let exit_code = error.exit_code();
    if let Err(print_error) = error.print() {
        // A closed pipe renders nothing: stay quiet instead of
        // reporting the failed render, but keep clap's own exit
        // code so a usage error never reports success.
        if print_error.kind() != std::io::ErrorKind::BrokenPipe {
            eprintln!(
                "{COMMAND_NAME}: failed to print command-line error: {print_error}"
            );
        }
    }
    exit_code
}

pub(crate) fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Manage project notes via their ^prj tasks")
        .long_about(
            "Manage Bob project notes through the completion-criteria task \
anchored with ^prj.\n\n\
The list subcommand is read-only: it scans project notes, validates leftover \
scheduled: YYYY-MM-DD frontmatter and inline [scheduled:: YYYY-MM-DD] on \
^prj, counts open #task items, counts open non-hidden tasks, and shows the \
current ^prj state. The sync subcommand updates project status, hoists leftover \
frontmatter scheduled onto ^prj, propagates that date onto ordinary tasks, \
and manages the single Sub-projects line from the ^prj task.",
        )
        .after_help(
            "Examples:\n  bob projects list\n  bob projects sync --dry-run\n  bob projects sync -b ~/bob",
        )
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(list_command())
        .subcommand(sync_command())
}

fn list_command() -> ClapCommand {
    ClapCommand::new("list")
        .about("List project notes and ^prj task state")
        .long_about(
            "List project notes, their lifecycle tasks, direct sub-projects, and \
            the health of each generated Projects catalog. The human table remains \
            the default; --format json prints the versioned report used by \
            Bob Ledger Tools. A missing or stale catalog is reported as a warning \
            and can be repaired with `bob projects sync`; a damaged catalog is an \
            error because it cannot be read or safely repaired.",
        )
        .after_help(
            "Examples:\n  bob projects list\n  bob projects list -f json\n  bob projects list --bob-dir ~/bob\n  bob projects list -b /tmp/bob-vault -f json",
        )
        .arg(bob_dir_arg())
        .arg(format_arg())
}

fn sync_command() -> ClapCommand {
    ClapCommand::new("sync")
        .about("Sync project status, task schedules, and sub-projects")
        .long_about(
            "Sync Bob project notes from the completion-criteria task anchored \
with ^prj.\n\n\
A checked ^prj task sets frontmatter status to done. A canceled ^prj task sets \
status to canceled. Active projects with no non-hidden open tasks and no \
open sub-projects have the #hide tag removed from their open ^prj task so \
it surfaces in dash.md's Tasks section; projects with non-hidden open tasks \
or open sub-projects get #hide added back. Sync also maintains a single \
Sub-projects line nested under open ^prj tasks after the whole Depends-On or \
Cancel Log subtree. Leftover scheduled: YYYY-MM-DD frontmatter is hoisted onto \
^prj as [scheduled:: YYYY-MM-DD] and the key is deleted when ^prj is open \
(frontmatter wins when both exist). Terminal projects are skipped silently. \
A missing ^prj leaves the key in place with the 'cannot hoist leftover \
scheduled onto ^prj' warning; malformed or duplicate ^prj tasks are project \
issues and make sync exit 1. \
When ^prj has a valid inline date, it overrides the normal surfacing rule: \
every open ordinary task receives a matching [scheduled:: YYYY-MM-DD] field \
unless it already has a valid equal or later schedule, and ordinary-task \
#hide tags are removed. Future dates keep exactly one #hide on ^prj; today \
and past dates preserve ^prj visibility unless it is the note's only task. \
Run `bob task reconcile` afterward to reconcile derived [?] Blocked markers. \
Invalid dates are reported and that file is left untouched.",
        )
        .after_help(
            "Examples:\n  bob projects sync --dry-run\n  bob projects sync -d -b ~/bob\n  bob projects sync --bob-dir /tmp/bob-vault",
        )
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
}

fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .long("bob-dir")
        .short('b')
        .value_name("DIR")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .long("dry-run")
        .short('d')
        .action(ArgAction::SetTrue)
        .help("Preview changes without writing files")
}

fn format_arg() -> Arg {
    Arg::new("format")
        .long("format")
        .short('f')
        .value_name("FORMAT")
        .value_parser(["human", "json"])
        .default_value("human")
        .help("Output format: human or json")
}

fn run_list(matches: &ArgMatches) -> i32 {
    let bob_dir = bob_dir_from_matches(matches);

    let report = scan_projects(&bob_dir);
    let today = projects_list_today(&bob_dir);
    let typed_notes = walk_typed_notes(&bob_dir);
    let mut note_contents = BTreeMap::new();
    let mut typed_paths = HashSet::new();
    for note in typed_notes {
        let path = PathBuf::from(&note.path);
        typed_paths.insert(path.clone());
        if let Ok(contents) = fs::read_to_string(bob_dir.join(&path)) {
            note_contents.insert(path, contents);
        }
    }
    let link_resolver =
        crate::native::vault_links::VaultLinkResolver::new(bob_dir.clone());
    let note_index = link_resolver.note_index();
    let catalogs = catalog::plan_catalogs(&note_contents, note_index, today);

    let mut details = BTreeMap::new();
    let mut parents = BTreeMap::new();
    let mut children = BTreeMap::new();
    for project in &report.projects {
        if let Some(contents) = note_contents.get(&project.relative_path) {
            details.insert(
                project.relative_path.clone(),
                catalog::child_info(
                    &project.relative_path,
                    contents,
                    project,
                    note_index,
                    today,
                ),
            );
        }
        let parent = project
            .parent_target
            .as_deref()
            .and_then(|target| {
                note_index.resolve(Some(&project.relative_path), target)
            })
            .filter(|path| typed_paths.contains(path))
            .map(|path| display_slash(&path));
        parents.insert(project.relative_path.clone(), parent);
        let descendants = catalogs
            .host(&project.relative_path)
            .map(|host| {
                host.rows
                    .iter()
                    .map(|row| display_slash(&row.path))
                    .collect()
            })
            .unwrap_or_default();
        children.insert(project.relative_path.clone(), descendants);
    }

    let day_file = crate::native::pomodoro::day_file_for(&bob_dir);
    let absolute_day_file = if day_file.is_absolute() {
        day_file.clone()
    } else {
        bob_dir.join(&day_file)
    };
    let daily_relative = absolute_day_file
        .strip_prefix(&bob_dir)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| day_file.clone());
    let today_tasks =
        fs::read_to_string(&absolute_day_file).ok().map(|contents| {
            crate::native::plan_budget::today::today_tasks(
                &bob_dir,
                &daily_relative.to_string_lossy(),
                &contents,
            )
            .tasks
        });
    let today_counts = today_tasks.map(|tasks| {
        let mut counts = HashMap::new();
        for task in tasks {
            *counts.entry(task.path).or_insert(0usize) += 1;
        }
        counts
    });

    let styler = Styler::detect();
    let catalogs_needing_repair = catalogs
        .hosts
        .iter()
        .filter(|catalog| catalog.state != catalog::CatalogState::Current)
        .count();
    let json_format = matches
        .get_one::<String>("format")
        .is_some_and(|format| format == "json");
    if json_format {
        let rows = report
            .projects
            .iter()
            .map(|project| {
                let path = &project.relative_path;
                output::ProjectListRow {
                    project,
                    detail: details.get(path),
                    parent: parents.get(path).cloned().flatten(),
                    children: children.get(path).cloned().unwrap_or_default(),
                    today: today_counts.as_ref().map(|counts| {
                        counts.get(&display_slash(path)).copied().unwrap_or(0)
                    }),
                }
            })
            .collect::<Vec<_>>();
        let payload =
            output::project_list_json(today, &rows, &catalogs, &report.issues);
        println!("{}", payload);
    } else {
        print_project_list(&report.projects, catalogs_needing_repair, &styler);
    }

    for issue in &report.issues {
        eprintln!("{COMMAND_NAME}: {}", issue.display());
    }
    for host in catalogs
        .hosts
        .iter()
        .filter(|host| host.state == catalog::CatalogState::Damaged)
    {
        for issue in &host.issues {
            eprintln!("{COMMAND_NAME}: {}", issue.message);
        }
    }

    if report.issues.is_empty()
        && !catalogs
            .hosts
            .iter()
            .any(|host| host.state == catalog::CatalogState::Damaged)
    {
        0
    } else {
        1
    }
}

fn projects_list_today(bob_dir: &Path) -> NaiveDate {
    let day_file = crate::native::pomodoro::day_file_for(bob_dir);
    day_file
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(crate::native::pomodoro::parse_day_file_date)
        .unwrap_or_else(|| bob_env::current_datetime().date())
}

fn display_slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn run_sync(matches: &ArgMatches) -> i32 {
    let bob_dir = bob_dir_from_matches(matches);
    let dry_run = matches.get_flag("dry-run");

    let report = sync_projects(&bob_dir, dry_run);
    let styler = Styler::detect();
    print_sync_report(&report, dry_run, &styler);

    for issue in &report.issues {
        eprintln!("{COMMAND_NAME}: {}", issue.display());
    }

    if report.issues.is_empty() {
        0
    } else {
        1
    }
}

fn bob_dir_from_matches(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir)
}

mod catalog;
mod catalog_rows;
pub(crate) use catalog_rows::{
    glyph_prefix_len, parse_row, CatalogRowState, BLOCKED_GLYPH,
    CANCELED_GLYPH, DONE_GLYPH, SCHEDULED_GLYPH, WARNING_GLYPH,
};
pub(crate) mod edits;
mod model;
mod output;
mod scan;
mod surface;
mod sync;
mod tags;
#[cfg(test)]
mod tests;

use edits::*;
use model::*;
use output::*;
use scan::*;
use sync::*;
use tags::*;

pub(crate) use model::{
    Frontmatter, ProjectStatus, SubprojectDisplay, SubprojectState,
    WikilinkRef, WikilinkSpan,
};
#[allow(unused_imports)]
// Added now for the album parser and registry phases.
pub(crate) use scan::{
    frontmatter_has_type, frontmatter_is_album, frontmatter_is_legacy_record,
    frontmatter_is_record, frontmatter_top_level_value,
};
pub(crate) use scan::{
    frontmatter_is_area, frontmatter_is_article, frontmatter_is_notebook,
    frontmatter_is_project, frontmatter_scalar, frontmatter_type_names,
    frontmatter_value, is_excluded_directory, is_markdown_file,
    list_item_content, parse_frontmatter, parse_offclock_types,
    project_is_finished, project_name_aliases, trim_yaml_scalar,
    walk_typed_notes, wikilink_path_target, wikilink_spans_in_line,
    FrontmatterScalar, OffclockTypesParsed,
};
pub(crate) use surface::{
    project_surface, project_surface_with_closed_child, ProjectSurface,
};
pub(crate) use sync::{
    display_for_wikilink, subproject_displays_in_marker_line,
};
pub(crate) use tags::{contains_task_tag, has_trailing_prj_anchor};

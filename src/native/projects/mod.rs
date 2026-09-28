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
const SUBPROJECTS_MARKER_PREFIX: &str = "🧩 **Sub-projects:**";
const SUBPROJECTS_SEPARATOR: &str = "•";
const SUBPROJECT_FUTURE_SCHEDULE_MARKER: &str = "🗓️";
const SUBPROJECT_DONE_MARKER: &str = "✅";
const SUBPROJECT_CANCELED_MARKER: &str = "❌";

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
        eprintln!(
            "{COMMAND_NAME}: failed to print command-line error: {print_error}"
        );
    }
    exit_code
}

fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Manage project notes via their ^prj tasks")
        .long_about(
            "Manage Bob project notes through the completion-criteria task \
anchored with ^prj.\n\n\
The list subcommand is read-only: it scans project notes, validates optional \
scheduled: YYYY-MM-DD frontmatter, counts open #task items, counts open \
non-hidden tasks, and shows the current ^prj state. The sync subcommand \
updates project status, propagates task schedules, and manages the single \
Sub-projects line from the ^prj task.",
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
        .about("List project notes and their ^prj task state")
        .after_help(
            "Examples:\n  bob projects list\n  bob projects list --bob-dir ~/bob\n  bob projects list -b /tmp/bob-vault",
        )
        .arg(bob_dir_arg())
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
Sub-projects line nested directly under open ^prj tasks. When valid scheduled \
frontmatter is present, it overrides the normal surfacing rule: every open \
ordinary task receives a matching [scheduled:: YYYY-MM-DD] field unless it \
already has a valid equal or later schedule, and ordinary-task #hide tags are \
removed. Future dates keep exactly one #hide on ^prj; today and past dates \
preserve ^prj visibility unless it is the note's only task. Run \
`bob task-status-hooks` afterward to reconcile derived [?] Blocked markers. \
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

fn run_list(matches: &ArgMatches) -> i32 {
    let bob_dir = bob_dir_from_matches(matches);

    let report = scan_projects(&bob_dir);
    let styler = Styler::detect();
    print_project_list(&report.projects, &styler);

    for issue in &report.issues {
        eprintln!("{COMMAND_NAME}: {}", issue.display());
    }

    if report.issues.is_empty() {
        0
    } else {
        1
    }
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

mod edits;
mod model;
mod output;
mod scan;
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

pub(crate) use model::{Frontmatter, ProjectStatus};
pub(crate) use scan::{
    frontmatter_is_area, frontmatter_is_project, frontmatter_value,
    is_markdown_file, parse_frontmatter, trim_yaml_scalar,
};

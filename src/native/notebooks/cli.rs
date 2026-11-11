//! `bob notebooks` list, check, sync, and create commands.

use std::{ffi::OsString, path::PathBuf};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};

use super::{create::CreateRequest, create_run, output, scan, sync};
use crate::native::{env as bob_env, records::cli::open_in_obsidian};

const COMMAND_NAME: &str = "bob notebooks";
const SCHEMA_VERSION: u32 = 1;

fn print_clap_error(error: clap::Error) -> i32 {
    let code = error.exit_code();
    if let Err(print_error) = error.print()
        && print_error.kind() != std::io::ErrorKind::BrokenPipe
    {
        eprintln!(
            "{COMMAND_NAME}: failed to print command-line error: {print_error}"
        );
    }
    code
}

pub(crate) fn run_list(args: Vec<OsString>) -> i32 {
    let mut command = build_cli_list();
    let name = command.get_name().to_owned();
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(name)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };
    execute_list(&matches)
}

pub(crate) fn run_check(args: Vec<OsString>) -> i32 {
    let mut command = build_cli_check();
    let name = command.get_name().to_owned();
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(name)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };
    execute_check(&matches)
}

pub(crate) fn run_sync(args: Vec<OsString>) -> i32 {
    let mut command = build_cli_sync();
    let name = command.get_name().to_owned();
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(name)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };
    execute_sync(&matches)
}

pub(crate) fn run_create(args: Vec<OsString>) -> i32 {
    let mut command = build_cli_create();
    let name = command.get_name().to_owned();
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(name)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };
    execute_create(&matches)
}

pub(crate) fn build_cli_list() -> ClapCommand {
    ClapCommand::new("bob notebooks list")
        .about("List notebooks grouped by root (default)")
        .long_about(
            "Tree of notebooks grouped by root, with titles, summaries, counts, and issue markers. This command is read-only. Bare `bob notebooks` runs this command.",
        )
        .after_help(
            "Examples:\n  bob notebooks\n  bob notebooks list\n  bob notebooks list -r dev -f json",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(root_arg())
}

pub(crate) fn build_cli_check() -> ClapCommand {
    ClapCommand::new("bob notebooks check")
        .about("Check notebooks against the contract")
        .long_about(
            "Findings grouped by notebook, plus a notebook/root/error/warning summary. Exits 1 on any error, 0 otherwise. This command is read-only.",
        )
        .after_help("Examples:\n  bob notebooks check\n  bob notebooks check -f json")
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
}

pub(crate) fn build_cli_sync() -> ClapCommand {
    ClapCommand::new("bob notebooks sync")
        .about("Write every generated notebook catalog")
        .long_about(
            "Repair the generated `## Notebooks` catalog in every area or project with notebooks beneath it. A real run takes the shared vault lock, pre-syncs, rewrites only the bytes between each catalog's markers, and makes one scoped commit `notebooks: sync catalogs (N notes)`; a vault whose catalogs are current writes and commits nothing. `--dry-run` prints a unified diff per host and changes nothing. A damaged catalog (a lone, duplicated, or reversed marker) is skipped and reported, and the command exits 1.",
        )
        .after_help(
            "Examples:\n  bob notebooks sync\n  bob notebooks sync -d\n  bob notebooks sync -f json\n\nSee docs/notebooks.md for the catalog contract.",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(format_arg())
        .arg(help_arg())
}

pub(crate) fn build_cli_create() -> ClapCommand {
    ClapCommand::new("bob notebooks create")
        .about("Create a notebook and its catalog rows")
        .long_about(
            "Create PARENT_ID.md under an area, a project of any status, or another notebook, and update every affected catalog in one guarded write and one commit. PARENT names the parent note; ID uses the block-ID alphabet (letters, digits, `-`), and the file is named `<parent>_<id>.md` with hyphens turned into underscores. `-s` is required; a summary over 140 characters only warns. The title defaults to the humanized ID. Validation or refusal exits 2.",
        )
        .after_help(
            "Examples:\n  bob notebooks create dev nvim -s 'Neovim config notes and plugin picks.'\n  bob notebooks create dev_nvim lua -s 'Lua for my config.' -t 'Lua'\n  bob notebooks create dev nvim -s 'Neovim notes.' -d\n  bob notebooks create dev nvim -s 'Neovim notes.' -o\n\nSee docs/notebooks.md for the creation contract.",
        )
        .disable_help_flag(true)
        .arg(
            Arg::new("parent")
                .value_name("PARENT")
                .required(true)
                .help("Parent area, project, or notebook (stem or path)"),
        )
        .arg(
            Arg::new("id")
                .value_name("ID")
                .required(true)
                .help("Block-style ID; names the file <parent>_<id>.md"),
        )
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(
            Arg::new("open")
                .long("open")
                .short('o')
                .action(ArgAction::SetTrue)
                .help("Open the new notebook in Obsidian after creating it"),
        )
        .arg(
            Arg::new("summary")
                .long("summary")
                .short('s')
                .value_name("SUMMARY")
                .required(true)
                .help("One-line summary of what belongs in the notebook"),
        )
        .arg(
            Arg::new("title")
                .long("title")
                .short('t')
                .value_name("TITLE")
                .help("One-line title; defaults to the humanized ID"),
        )
}

fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .long("dry-run")
        .short('d')
        .action(ArgAction::SetTrue)
        .help("Print what would change as unified diffs; write nothing")
}

fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .long("bob-dir")
        .short('b')
        .value_name("DIR")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

fn format_arg() -> Arg {
    Arg::new("format")
        .long("format")
        .short('f')
        .value_name("FORMAT")
        .default_value("human")
        .value_parser(["human", "json"])
        .help("Output format: human or json")
}

fn root_arg() -> Arg {
    Arg::new("root")
        .long("root")
        .short('r')
        .value_name("ROOT")
        .help("Filter to one computed root identity")
}

fn help_arg() -> Arg {
    Arg::new("help")
        .long("help")
        .short('h')
        .action(ArgAction::Help)
        .help("Show help")
}

fn vault_from(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir)
}

fn format_is_json(matches: &ArgMatches) -> bool {
    matches.get_one::<String>("format").map(String::as_str) == Some("json")
}

fn root_filter(matches: &ArgMatches) -> Option<String> {
    matches
        .get_one::<String>("root")
        .map(|value| normalize_root(value))
}

fn normalize_root(value: &str) -> String {
    let trimmed = value.trim();
    let without_md = trimmed
        .strip_suffix(".md")
        .or_else(|| trimmed.strip_suffix(".MD"))
        .unwrap_or(trimmed);
    without_md.to_string()
}

fn execute_list(matches: &ArgMatches) -> i32 {
    let bob_dir = vault_from(matches);
    let json = format_is_json(matches);
    let root = root_filter(matches);
    let output = scan::scan_notebooks(&bob_dir);
    for error in &output.io_errors {
        eprintln!("{COMMAND_NAME}: {error}");
    }
    if !output.io_errors.is_empty() && !json {
        return 1;
    }
    let mut report = output.report;
    if let Some(root) = root {
        report
            .notebooks
            .retain(|entry| entry.root.as_deref() == Some(root.as_str()));
        // Host catalogs filter by the host's own computed root, and the
        // counts follow the filtered notebooks and catalogs.
        report
            .catalogs
            .retain(|record| record.root.as_deref() == Some(root.as_str()));
        report.recount();
    }
    if !output.io_errors.is_empty() {
        // Never present I/O failures as a clean scan.
        let payload = report.to_json();
        println!("{payload}");
        return 1;
    }
    if json {
        println!("{}", report.to_json());
        0
    } else {
        output::print_list(&report, &bob_dir);
        0
    }
}

fn execute_check(matches: &ArgMatches) -> i32 {
    let bob_dir = vault_from(matches);
    let json = format_is_json(matches);
    let output = scan::scan_notebooks(&bob_dir);
    for error in &output.io_errors {
        eprintln!("{COMMAND_NAME}: {error}");
    }
    if !output.io_errors.is_empty() && !json {
        return 1;
    }
    let report = output.report;
    if !output.io_errors.is_empty() {
        let payload = report.to_json();
        println!("{payload}");
        return 1;
    }
    if json {
        println!("{}", report.to_json());
    } else {
        output::print_check(&report);
    }
    if report.counts.errors > 0 {
        1
    } else {
        0
    }
}

#[allow(dead_code)]
fn schema_version() -> u32 {
    SCHEMA_VERSION
}

fn print_json(payload: &serde_json::Value) {
    println!("{payload}");
}

fn execute_sync(matches: &ArgMatches) -> i32 {
    let bob_dir = vault_from(matches);
    let json = format_is_json(matches);
    let dry_run = matches.get_flag("dry-run");
    let report = if dry_run {
        sync::dry_run(&bob_dir)
    } else {
        sync::run_standalone(&bob_dir, json)
    };
    if json {
        print_json(&report.to_json());
    } else {
        sync::print_human(&report);
    }
    report.exit_code()
}

fn execute_create(matches: &ArgMatches) -> i32 {
    let bob_dir = vault_from(matches);
    let json = format_is_json(matches);
    let dry_run = matches.get_flag("dry-run");
    let text = |name: &str| {
        matches.get_one::<String>(name).cloned().unwrap_or_default()
    };
    let request = CreateRequest {
        parent: text("parent"),
        id: text("id"),
        summary: text("summary"),
        title: matches.get_one::<String>("title").cloned(),
        body: None,
        now: bob_env::current_datetime(),
        timezone: bob_env::configured_timezone(),
    };
    let mut report = if dry_run {
        create_run::dry_run(&bob_dir, &request)
    } else {
        create_run::run_standalone(&bob_dir, &request, json)
    };
    // Open only after a committed creation; failure to open is a warning
    // and never rolls the file back.
    if matches.get_flag("open")
        && !dry_run
        && report.failure.is_none()
        && let Some(plan) = &report.plan
        && let Err(message) = open_in_obsidian(&bob_dir, &plan.path)
    {
        report.warnings.push(message);
    }
    if json {
        print_json(&report.to_json());
    } else {
        create_run::print_human(&report);
    }
    report.exit_code()
}

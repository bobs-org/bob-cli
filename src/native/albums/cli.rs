//! `bob albums` list and check commands.

use std::{ffi::OsString, path::PathBuf};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};

use super::{output, registry::AlbumRegistry, report, sync};
use crate::native::{
    config, env as bob_env, notebooks::snapshot::VaultSnapshot,
};

const COMMAND_NAME: &str = "bob albums";

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

pub(crate) fn build_cli_list() -> ClapCommand {
    ClapCommand::new("bob albums list")
        .about("List albums grouped by root (default)")
        .long_about(
            "Albums grouped by root, with titles, record counts, group counts, freshness, and issue markers. The report also includes health for each generated `## Albums` owner catalog, including stale catalogs whose last album was removed. Run `bob albums sync` to repair catalog findings. This command is read-only. Bare `bob albums` runs this command. `bob albums create` ships in a later phase.",
        )
        .after_help(
            "Examples:\n  bob albums\n  bob albums list\n  bob albums list -r body -f json\n\nSee docs/records.md for the album contract.",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(root_arg())
}

pub(crate) fn build_cli_check() -> ClapCommand {
    ClapCommand::new("bob albums check")
        .about("Check albums against the contract")
        .long_about(
            "Findings grouped by album and by owner catalog, plus an album/record/error/warning summary. Catalog warnings and damaged-region errors appear after album findings. Exits 1 on any error, 0 otherwise. This command is read-only.",
        )
        .after_help(
            "Examples:\n  bob albums check\n  bob albums check -f json\n\nSee docs/records.md for the album contract.",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
}

pub(crate) fn build_cli_sync() -> ClapCommand {
    ClapCommand::new("bob albums sync")
        .about("Repair generated records indexes and album catalogs")
        .long_about(
            "Repair each album's generated [!records] index and each owner's generated `## Albums` catalog from one snapshot. A real run takes the shared vault lock, pre-syncs, guards album declarations, record frontmatter, folder membership, and the owner/ancestor/declaration inputs catalog rows use, then makes one scoped commit `albums: sync (N notes)` when indexes or catalogs change. Current indexes and catalogs write and commit nothing and skip post-sync. `--dry-run` plans diffs without a lock or sync cycle. Damaged albums and damaged catalogs are left intact, healthy regions continue, and any damaged region or runtime failure exits 1.",
        )
        .after_help(
            "Examples:\n  bob albums sync\n  bob albums sync -d\n  bob albums sync -b ~/bob-work -f json\n\nSee docs/records.md for the index, catalog, and sync contract.",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(format_arg())
        .arg(help_arg())
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
        .help("Print planned unified diffs; write nothing")
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

fn load_report(bob_dir: &std::path::Path) -> report::AlbumReport {
    let config_path = config::config_path();
    let snapshot = VaultSnapshot::load_with_config(bob_dir, &config_path);
    let registry =
        AlbumRegistry::from_snapshot(bob_dir, &config_path, &snapshot);
    report::build_report(&registry, &snapshot)
}

fn print_io_errors(errors: &[String]) {
    for error in errors {
        eprintln!("{COMMAND_NAME}: {error}");
    }
}

fn execute_list(matches: &ArgMatches) -> i32 {
    let bob_dir = vault_from(matches);
    let json = format_is_json(matches);
    let mut report = load_report(&bob_dir);
    print_io_errors(&report.io_errors);
    if !report.io_errors.is_empty() && !json {
        return 1;
    }
    if let Some(root) = root_filter(matches) {
        report.filter_root(&root);
    }
    if !report.io_errors.is_empty() {
        println!("{}", report.to_json());
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
    let report = load_report(&bob_dir);
    print_io_errors(&report.io_errors);
    if !report.io_errors.is_empty() && !json {
        return 1;
    }
    if !report.io_errors.is_empty() {
        println!("{}", report.to_json());
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

fn execute_sync(matches: &ArgMatches) -> i32 {
    let bob_dir = vault_from(matches);
    let json = format_is_json(matches);
    let report = if matches.get_flag("dry-run") {
        sync::dry_run(&bob_dir)
    } else {
        sync::run_standalone(&bob_dir, json)
    };
    if json {
        println!("{}", report.to_json());
    } else {
        sync::print_human(&report);
    }
    report.exit_code()
}

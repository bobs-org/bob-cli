//! `bob ref migrate-zorg`: dry-run planner and report.
//!
//! The bare command is a read-only dry run: no lock, no sync, no writes.
//! It plans one legacy note per unmirrored record (books fold their
//! chapters) and prints the human or JSON report.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Arg, ArgAction, ArgMatches, Command as ClapCommand};

use super::super::output::{print_find_error, Format};
use super::super::{build_index, LibraryConfig};
use super::plan::plan_migration;
use super::report::{print_json, render_human};
use crate::native::env as bob_env;
use crate::native::highlights_ref::{
    bob_dir_arg, configured_path, ref_dir_arg, ENV_REF_DIR,
};

/// The `bob ref migrate-zorg` subcommand builder.
pub(crate) fn migrate_zorg_command() -> ClapCommand {
    ClapCommand::new("migrate-zorg")
        .about(
            "Migrate zorg-era reading records into ref/zorg/ (dry run)",
        )
        .arg(bob_dir_arg())
        .arg(
            Arg::new("format")
                .long("format")
                .short('f')
                .value_name("FORMAT")
                .value_parser(["human", "json"])
                .default_value("human")
                .help("Output format"),
        )
        .arg(
            Arg::new("offline")
                .long("offline")
                .short('o')
                .action(ArgAction::SetTrue)
                .help("Skip vault sync cycles (the dry run performs no sync)"),
        )
        .arg(ref_dir_arg())
        .after_help(
            "The bare command only plans: it prints where every unmirrored zorg-era status:: record would migrate under ref/zorg/ and changes nothing.\n\
            \n\
            Examples:\n  \
            bob ref migrate-zorg\n  \
            bob ref migrate-zorg -f json",
        )
}

/// Library locations from the `migrate-zorg` matches: only `bob-dir`
/// and `ref-dir` are read.
pub(crate) fn migrate_config_from_matches(
    matches: &ArgMatches,
) -> LibraryConfig {
    let bob_dir = matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir);
    let ref_dir =
        configured_path(matches, "ref-dir", ENV_REF_DIR, "ref", &bob_dir);
    LibraryConfig {
        xlib_dir: bob_dir.join("xlib"),
        bob_dir,
        ref_dir,
    }
}

/// Run `bob ref migrate-zorg`: build the index, plan, and render.
pub(crate) fn run_migrate_zorg(matches: &ArgMatches) -> i32 {
    let format = Format::from_name(
        matches
            .get_one::<String>("format")
            .map(String::as_str)
            .unwrap_or("human"),
    );
    let config = migrate_config_from_matches(matches);
    let index = match build_index(&config) {
        Ok(index) => index,
        Err(error) => {
            print_find_error(
                format,
                "ref migrate-zorg",
                "missing_ref_dir",
                &error.to_string(),
                Some("pass -b/--bob-dir or -r/--ref-dir, or set BOB_DIR"),
            );
            return 1;
        }
    };
    let plan = plan_migration(&config.bob_dir, &config.ref_dir, &index.rows);
    match format {
        Format::Json => print_json(&plan),
        // Clap restricts `--format` to human|json, so anything else is
        // the human report.
        Format::Human | Format::Markdown => print!("{}", render_human(&plan)),
    }
    0
}

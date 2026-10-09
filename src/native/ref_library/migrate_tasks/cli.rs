//! `bob ref migrate-tasks`: dry-run planner, report, and `--write`.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Arg, ArgAction, ArgMatches, Command as ClapCommand};

use super::super::{
    output::generated_at, output::REF_SCHEMA_VERSION, LibraryConfig,
};
use super::plan::{parse_map_file, plan_migration};
use super::report::{print_json, render_human, render_tsv, ReportMode};
use crate::native::env as bob_env;
use crate::native::highlights_ref::{
    bob_dir_arg, configured_path, ref_dir_arg, ENV_REF_DIR,
};

/// The `bob ref migrate-tasks` subcommand builder.
pub(crate) fn migrate_tasks_command() -> ClapCommand {
    ClapCommand::new("migrate-tasks")
        .about("Move open ref tasks into parent notes; dry run unless --write")
        .arg(bob_dir_arg())
        .arg(
            Arg::new("format")
                .long("format")
                .short('f')
                .value_name("FORMAT")
                .value_parser(["human", "json", "tsv"])
                .default_value("human")
                .help("Output format"),
        )
        .arg(
            Arg::new("map")
                .long("map")
                .short('m')
                .value_name("FILE")
                .value_parser(clap::value_parser!(OsString))
                .help("TSV map of ref_note to parent"),
        )
        .arg(
            Arg::new("offline")
                .long("offline")
                .short('o')
                .action(ArgAction::SetTrue)
                .help("Skip both vault-sync cycles; commit locally without pushing"),
        )
        .arg(ref_dir_arg())
        .arg(
            Arg::new("write")
                .long("write")
                .short('w')
                .action(ArgAction::SetTrue)
                .help("Apply the planned migration as one revertible commit"),
        )
        .after_help(
            "The bare command only plans: it prints where every open v1 ref task would move and changes nothing. Pass -m map.tsv to override parents, -f tsv to edit the map round trip, and --write to apply the plan as one scoped commit; undo it with `git revert`.\n\
            \n\
            Examples:\n  \
            bob ref migrate-tasks\n  \
            bob ref migrate-tasks -f tsv\n  \
            bob ref migrate-tasks -m map.tsv --write --offline",
        )
}

/// Library locations from the `migrate-tasks` matches.
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

fn fail_json(code: &str, message: &str, hint: Option<&str>) -> i32 {
    let envelope = serde_json::json!({
        "ok": false,
        "schema_version": REF_SCHEMA_VERSION,
        "command": "ref migrate-tasks",
        "generated_at": generated_at(),
        "error": {
            "code": code,
            "message": message,
            "hint": hint,
        },
    });
    println!(
        "{}",
        serde_json::to_string(&envelope).expect("error serializes")
    );
    1
}

fn fail(format: &str, code: &str, message: &str, hint: Option<&str>) -> i32 {
    if format == "json" {
        return fail_json(code, message, hint);
    }
    eprintln!("bob ref migrate-tasks: error: {message}");
    if let Some(hint) = hint {
        eprintln!("hint: {hint}");
    }
    let _ = code;
    1
}

/// Run `bob ref migrate-tasks`.
pub(crate) fn run_migrate_tasks(matches: &ArgMatches) -> i32 {
    let format = matches
        .get_one::<String>("format")
        .map(String::as_str)
        .unwrap_or("human")
        .to_string();
    if matches.get_flag("write") {
        return super::write::run_migrate_tasks_write(matches, format);
    }
    let config = migrate_config_from_matches(matches);
    if !config.ref_dir.is_dir() {
        return fail(
            &format,
            "missing_ref_dir",
            &format!(
                "reference directory not found: {}",
                config.ref_dir.display()
            ),
            Some("pass -b/--bob-dir or -r/--ref-dir, or set BOB_DIR"),
        );
    }
    let map = match load_map(matches) {
        Ok(map) => map,
        Err(message) => {
            return fail(&format, "map", &message, None);
        }
    };
    let plan = plan_migration(&config.bob_dir, &config.ref_dir, &map);
    match format.as_str() {
        "json" => print_json(&plan, &ReportMode::dry_run(), None),
        "tsv" => print!("{}", render_tsv(&plan)),
        _ => print!("{}", render_human(&plan, &ReportMode::dry_run())),
    }
    0
}

pub(crate) fn load_map(
    matches: &ArgMatches,
) -> Result<BTreeMap<String, String>, String> {
    let Some(path_os) = matches.get_one::<OsString>("map") else {
        return Ok(BTreeMap::new());
    };
    let path = PathBuf::from(path_os);
    let contents = std::fs::read_to_string(&path)
        .map_err(|e| format!("read map {}: {e}", path.display()))?;
    Ok(parse_map_file(&contents))
}

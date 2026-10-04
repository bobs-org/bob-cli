use std::{
    ffi::OsString,
    iter,
    path::{Path, PathBuf},
};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde_json::json;

use crate::native::{env as bob_env, style::Styler};

use super::{
    git::{print_pull_outcome, pull_repo},
    model::{SyncOptions, COMMAND_NAME},
    render::{print_plugins_table, print_sync_report, success_json},
    scan::scan_plugins,
    sync::sync_plugins,
};

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
        // No subcommand defaults to `list`; top-level matches carry the same
        // options so `bob plugins -f json` works without typing `list`.
        None => run_list(&matches),
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

pub(crate) fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Manage Bob Obsidian plugins from the bob-plugins repo")
        .long_about(
            "Manage Bryan's custom Bob Obsidian plugins from the \
bob-plugins repo.\n\n\
The list subcommand is read-only: it reads each plugin manifest from the repo, \
byte-compares the managed files against the vault copy to report sync state, \
and reads community-plugins.json to report whether the vault has the plugin \
enabled. Running `bob plugins` with no subcommand runs list.\n\n\
Before list or sync analyzes files, the plugins repo is refreshed with a \
non-interactive `git pull` unless --no-pull is given. Pull failures warn and \
continue with the existing checkout.\n\n\
The sync subcommand deploys the repo into the vault: it copies the managed \
files (manifest.json, main.js, and styles.css) from the repo into the vault \
plugin folder, never touching data.json or other runtime files. It refuses to \
overwrite a vault file that has uncommitted changes in the vault Git repo \
unless --force is given. For files that would change, sync prints a diff, and \
every overwritten vault file is backed up before it is replaced.",
        )
        .after_help(
            "Examples:\n  bob plugins\n  bob plugins list\n  bob plugins list -f json\n  bob plugins sync --dry-run\n  bob plugins sync --no-pull --dry-run\n  bob plugins sync -p bob-project-tasks",
        )
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(no_pull_arg())
        .arg(repo_arg())
        .subcommand(list_command())
        .subcommand(sync_command())
}

fn list_command() -> ClapCommand {
    ClapCommand::new("list")
        .about("List Bob plugins with repo version and vault sync state (default)")
        .after_help(
            "Examples:\n  bob plugins list\n  bob plugins list -f json\n  bob plugins list --no-pull\n  bob plugins list -b ~/bob -r ~/projects/github/bobs-org/bob-plugins",
        )
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(no_pull_arg())
        .arg(repo_arg())
}

fn sync_command() -> ClapCommand {
    ClapCommand::new("sync")
        .about("Deploy repo plugin files into the vault")
        .long_about(
            "Deploy Bob plugins from the bob-plugins repo into the vault.\n\n\
For each plugin, the managed files (manifest.json, main.js, and styles.css \
when present) are copied from <repo>/plugins/<id>/ into \
<bob-dir>/.obsidian/plugins/<id>/. Runtime files such as data.json are never \
touched. A vault file that has uncommitted changes in the vault Git repo is \
left alone with a warning unless --force is given, so local edits are never \
clobbered silently. Files that would change are shown as unified diffs; \
overwritten vault files are copied to a timestamped backup directory first. \
Files that already match the repo are reported as unchanged. The plugins repo \
is refreshed with a non-interactive `git pull` before analysis unless \
--no-pull is given.",
        )
        .after_help(
            "Examples:\n  bob plugins sync --dry-run\n  bob plugins sync --no-pull --dry-run\n  bob plugins sync -p bob-project-tasks\n  bob plugins sync -F -b ~/bob -r ~/projects/github/bobs-org/bob-plugins",
        )
        .arg(backup_dir_arg())
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(force_arg())
        .arg(no_pull_arg())
        .arg(plugin_arg())
        .arg(repo_arg())
}

fn backup_dir_arg() -> Arg {
    Arg::new("backup-dir")
        .long("backup-dir")
        .short('B')
        .value_name("DIR")
        .value_parser(OsStringValueParser::new())
        .help(
            "Directory for backups of overwritten vault files; defaults to \
BOB_PLUGIN_BACKUPS_DIR or ~/.local/state/bob-cli/plugin-backups",
        )
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
        .value_parser(["table", "json"])
        .default_value("table")
        .help("Output format: table or json")
}

fn repo_arg() -> Arg {
    Arg::new("repo")
        .long("repo")
        .short('r')
        .value_name("DIR")
        .value_parser(OsStringValueParser::new())
        .help(
            "Plugins repo root; defaults to BOB_PLUGINS_DIR or \
~/projects/github/bobs-org/bob-plugins",
        )
}

fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .long("dry-run")
        .short('d')
        .action(ArgAction::SetTrue)
        .help("Preview the copies without writing any files")
}

fn force_arg() -> Arg {
    Arg::new("force")
        .long("force")
        .short('F')
        .action(ArgAction::SetTrue)
        .help("Overwrite vault files with uncommitted Git changes")
}

fn no_pull_arg() -> Arg {
    Arg::new("no-pull")
        .long("no-pull")
        .short('n')
        .action(ArgAction::SetTrue)
        .help("Skip 'git pull' on the plugins repo before analyzing")
}

fn plugin_arg() -> Arg {
    Arg::new("plugin")
        .long("plugin")
        .short('p')
        .value_name("ID")
        .help("Sync only this plugin id; defaults to every plugin")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Table,
    Json,
}

impl OutputFormat {
    fn from_matches(matches: &ArgMatches) -> Self {
        match matches
            .get_one::<String>("format")
            .map(String::as_str)
            .unwrap_or("table")
        {
            "json" => Self::Json,
            _ => Self::Table,
        }
    }
}

fn repo_from_matches(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("repo")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::plugins_dir)
}

fn bob_dir_from_matches(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir)
}

fn backup_dir_from_matches(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("backup-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::plugin_backups_dir)
}

fn run_list(matches: &ArgMatches) -> i32 {
    let repo = repo_from_matches(matches);
    maybe_pull_repo(matches, &repo);
    let bob_dir = bob_dir_from_matches(matches);
    let report = scan_plugins(&repo, &bob_dir);

    match OutputFormat::from_matches(matches) {
        OutputFormat::Table => {
            let styler = Styler::detect();
            print_plugins_table(&report, &styler);
            for issue in &report.issues {
                eprintln!("{COMMAND_NAME}: {issue}");
            }
        }
        OutputFormat::Json => {
            if report.issues.is_empty() {
                println!("{}", success_json(&report.result()));
            } else {
                println!(
                    "{}",
                    json!({ "ok": false, "error": report.issue_summary() })
                );
            }
        }
    }

    // Drift and not-installed are reportable states, not errors; only a real
    // failure such as an unreadable repo sets a non-zero exit.
    if report.issues.is_empty() {
        0
    } else {
        1
    }
}

fn run_sync(matches: &ArgMatches) -> i32 {
    let repo = repo_from_matches(matches);
    maybe_pull_repo(matches, &repo);
    let timestamp = bob_env::current_datetime().format("%Y%m%d-%H%M%S");
    let options = SyncOptions {
        repo,
        bob_dir: bob_dir_from_matches(matches),
        backup_run_dir: backup_dir_from_matches(matches)
            .join(timestamp.to_string()),
        only: matches.get_one::<String>("plugin").cloned(),
        dry_run: matches.get_flag("dry-run"),
        force: matches.get_flag("force"),
    };

    let report = sync_plugins(&options);
    let styler = Styler::detect();
    print_sync_report(&report, options.dry_run, &styler);
    for issue in &report.issues {
        eprintln!("{COMMAND_NAME}: {issue}");
    }

    // A refused dirty file is a deliberate warning, not a failure; only a real
    // error such as an unreadable repo or a failed copy sets a non-zero exit.
    if report.issues.is_empty() {
        0
    } else {
        1
    }
}

fn maybe_pull_repo(matches: &ArgMatches, repo: &Path) {
    if matches.get_flag("no-pull") {
        return;
    }
    print_pull_outcome(pull_repo(repo));
}

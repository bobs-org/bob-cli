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

use crate::native::{
    config::{display_config_path, load_plugins_config},
    env as bob_env,
    style::Styler,
};

use super::{
    git::{print_pull_outcome, pull_repo},
    guard::resolve_default_repo,
    model::{RepoSource, SyncOptions, COMMAND_NAME},
    render::{
        print_plugins_table, print_sync_report, success_json, sync_success_json,
    },
    scan::{plugin_ids_in_repo, scan_plugins},
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
plugin folder, never touching data.json or other runtime files. Sync deploys \
only the plugin ids listed in `plugins.sync` in bob's config file; a missing \
list exits 2 with a ready-to-paste snippet. Each real \
deploy records a `.bob-deploy.json` provenance stamp beside the plugin. Sync \
refuses to overwrite another checkout's uncommitted or non-ancestor deploy \
unless --force is given, except that a clean checkout overwrites a dirty \
deploy from another checkout or host without --force when one commit reachable \
from its HEAD holds the stamped bundle byte for byte; the sync names that \
proving commit. Sync also refuses to overwrite a vault file that \
has uncommitted changes in the vault Git repo unless --force is given. For \
files that would change, sync prints a diff, and every overwritten vault file \
is backed up before it is replaced. In a Git vault, sync and list also warn \
when a plugin's whole vault folder is not ignored or holds tracked files; \
the warning rides along in table and JSON output and never edits .gitignore.",
        )
        .after_help(
            "Examples:\n  bob plugins\n  bob plugins list\n  bob plugins list -f json\n  bob plugins sync --dry-run\n  bob plugins sync -d\n  bob plugins sync --no-pull --dry-run\n  bob plugins sync -p bob-project-tasks",
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
        .long_about(
            "List Bob plugins with repo version and vault sync state.\n\n\
In a Git vault, list also warns when a plugin's whole vault folder \
(.obsidian/plugins/<id>/) is not ignored or holds tracked files. The warning \
prints under the plugin row and appears as `vault_ignore` in JSON output; it \
never edits .gitignore and never changes the SYNC/VAULT columns. Rows whose \
id is left out of `plugins.sync` in bob's config file carry `configured: \
false` and a detail line noting that `bob plugins sync` skips them.",
        )
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
Sync deploys only the plugin ids listed in `plugins.sync` in bob's config \
file; a missing list exits 2 with a ready-to-paste snippet. \
For each listed plugin, the managed files (manifest.json, main.js, and \
styles.css when present) are copied from <repo>/plugins/<id>/ into \
<bob-dir>/.obsidian/plugins/<id>/. Runtime files such as data.json are never \
touched. Each real deploy writes a `.bob-deploy.json` stamp recording the \
source checkout, HEAD, dirty state, host, time, and managed-file hashes. In a \
Git vault that stamp must be ignored (for example \
`.obsidian/plugins/*/.bob-deploy.json`); --force does not bypass a tracked or \
unignored stamp. Sync refuses to overwrite a deploy from another checkout's \
uncommitted build, a non-ancestor clean revision, or unknown/incomplete \
provenance unless --force is given, except that a clean checkout overwrites a \
dirty deploy from another checkout or host without --force when one commit \
reachable from its HEAD holds the stamped bundle byte for byte; the sync \
names that proving commit and mixed per-file matches from different commits \
still refuse. A vault file that has uncommitted changes \
in the vault Git repo is left alone with a warning unless --force is given, \
so local edits are never clobbered silently. Files that would change are \
shown as unified diffs; overwritten vault files are copied to a timestamped \
backup directory first. Files that already match the repo are reported as \
unchanged. In a Git vault, sync also warns when a plugin's whole vault folder \
(.obsidian/plugins/<id>/) is not ignored or holds tracked files; the warning \
prints under the plugin row and appears as `vault_ignore` in JSON output, in \
dry-run too, and never blocks the copy, edits .gitignore, or untracks files. \
Sync also warns when a deployed plugin id is missing from the vault's \
.obsidian/community-plugins.json; the warning prints under the plugin row and \
appears as `disabled_warning` in JSON output, in dry-run too, and never blocks \
the copy or edits the vault config. List already reports enablement in its \
VAULT column; only sync gains this warning. \
The plugins repo is refreshed with a non-interactive `git pull` \
before analysis unless --no-pull is given; when the pull fails the refresh \
falls back to `git fetch` so remote-tracking refs stay fresh while checkout \
files are left untouched. Pass `-f json` to print a single \
machine-readable object instead of the table and diffs. The repo root \
resolves from --repo, then the enclosing bob-plugins checkout when the cwd \
sits inside one, then BOB_PLUGINS_DIR, then the default checkout.",
        )
        .after_help(
            "Examples:\n  bob plugins sync --dry-run\n  bob plugins sync -d\n  bob plugins sync --dry-run -f json\n  bob plugins sync --no-pull --dry-run\n  bob plugins sync -p bob-project-tasks\n  bob plugins sync -F -b ~/bob -r ~/projects/github/bobs-org/bob-plugins",
        )
        .arg(backup_dir_arg())
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(force_arg())
        .arg(format_arg())
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
            "Plugins repo root; defaults to the enclosing bob-plugins checkout, \
then BOB_PLUGINS_DIR or ~/projects/github/bobs-org/bob-plugins",
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
        .help(
            "Overwrite a protected plugin deploy or vault files with \
uncommitted Git changes",
        )
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
        .help(
            "Sync only this plugin id, which must also be listed in plugins.sync; defaults to every listed plugin",
        )
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

fn repo_from_matches(matches: &ArgMatches) -> (PathBuf, RepoSource) {
    if let Some(flag) = matches.get_one::<OsString>("repo") {
        return (
            bob_env::expand_tilde(&PathBuf::from(flag)),
            RepoSource::Flag,
        );
    }
    let resolved = resolve_default_repo();
    if let Some(notice) = resolved.notice {
        eprintln!("{notice}");
    }
    (resolved.repo, resolved.source)
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
    let (repo, repo_source) = repo_from_matches(matches);
    let config_path = crate::native::config::config_path();
    let (configured, notice) = match load_plugins_config(&config_path) {
        Ok(config) => match config.sync {
            Some(ids) => (
                Some(
                    ids.into_iter().collect::<std::collections::BTreeSet<_>>(),
                ),
                None,
            ),
            None => (None, Some(unset_list_notice())),
        },
        Err(error) => (None, Some(error.to_string())),
    };
    maybe_pull_repo(matches, &repo);
    let bob_dir = bob_dir_from_matches(matches);
    let report =
        scan_plugins(&repo, &bob_dir, repo_source, configured.as_ref());

    match OutputFormat::from_matches(matches) {
        OutputFormat::Table => {
            let styler = Styler::detect();
            print_plugins_table(&report, &styler, configured.is_some());
            if let Some(notice) = &notice {
                eprintln!("{COMMAND_NAME}: {notice}");
            }
            for issue in &report.issues {
                eprintln!("{COMMAND_NAME}: {issue}");
            }
        }
        OutputFormat::Json => {
            if let Some(notice) = &notice {
                eprintln!("{COMMAND_NAME}: {notice}");
            }
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

fn unset_list_notice() -> String {
    format!(
        "{} has no plugins.sync list; bob plugins sync deploys nothing until you add one",
        display_config_path()
    )
}

fn unset_sync_message(repo: &Path) -> String {
    let mut message = format!(
        "{} has no plugins.sync list; bob plugins sync deploys only the ids listed in plugins.sync there. Add one, for example:",
        display_config_path()
    );
    let ids = plugin_ids_in_repo(repo);
    let readable = !ids.is_empty();
    let ids = if readable {
        ids
    } else {
        vec!["<plugin-id>".to_string()]
    };
    message.push_str("\nplugins:\n  sync:");
    for id in &ids {
        message.push_str(&format!("\n    - {id}"));
    }
    if !readable {
        message
            .push_str("\nRun `bob plugins list` to see the repo plugin ids.");
    }
    message
}

fn exit_config_error(message: String, matches: &ArgMatches) -> i32 {
    match OutputFormat::from_matches(matches) {
        OutputFormat::Table => {
            eprintln!("{COMMAND_NAME}: {message}");
        }
        OutputFormat::Json => {
            println!("{}", json!({ "ok": false, "error": message }));
        }
    }
    2
}

fn run_sync(matches: &ArgMatches) -> i32 {
    let (repo, repo_source) = repo_from_matches(matches);
    let config_path = crate::native::config::config_path();
    let configured = match load_plugins_config(&config_path) {
        Ok(config) => match config.sync {
            Some(ids) => ids,
            None => {
                return exit_config_error(unset_sync_message(&repo), matches);
            }
        },
        Err(error) => {
            return exit_config_error(error.to_string(), matches);
        }
    };
    maybe_pull_repo(matches, &repo);
    let timestamp = bob_env::current_datetime().format("%Y%m%d-%H%M%S");
    let options = SyncOptions {
        repo,
        repo_source,
        bob_dir: bob_dir_from_matches(matches),
        backup_run_dir: backup_dir_from_matches(matches)
            .join(timestamp.to_string()),
        only: matches.get_one::<String>("plugin").cloned(),
        dry_run: matches.get_flag("dry-run"),
        force: matches.get_flag("force"),
        configured: configured.into_iter().collect(),
        config_path,
    };

    let report = sync_plugins(&options);
    match OutputFormat::from_matches(matches) {
        OutputFormat::Table => {
            let styler = Styler::detect();
            print_sync_report(&report, options.dry_run, &styler);
            for issue in &report.issues {
                eprintln!("{COMMAND_NAME}: {issue}");
            }
        }
        OutputFormat::Json => {
            if report.issues.is_empty() {
                println!(
                    "{}",
                    sync_success_json(&report.result(options.dry_run))
                );
            } else {
                println!(
                    "{}",
                    json!({ "ok": false, "error": report.issue_summary() })
                );
            }
        }
    }

    // A skipped dirty vault file is a deliberate warning, not a failure.
    // Deploy-protection refusals, unreadable repos, and failed copies set a
    // non-zero exit.
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

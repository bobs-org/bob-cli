//! `bob gkeep` command tree, help text, and typed arguments.
//!
//! This module pins the whole CLI surface: every subcommand, option,
//! default, and help block. Later phases implement the handlers; they
//! change only `doctor.rs`, `list.rs`, `login.rs`, and `pull.rs`.

use std::{ffi::OsString, path::PathBuf};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};

use crate::native::env as bob_env;

const COMMAND_NAME: &str = "bob gkeep";

/// The full `bob gkeep` command tree.
pub(crate) fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Drain the Google Keep inbox into Obsidian tasks")
        .long_about(
            "Google Keep → Obsidian inbox drain: move every Keep inbox \
             note into gkeep_inbox.md as Obsidian tasks and archive each \
             note in Keep only after its current content is provably in \
             the vault.",
        )
        .after_help(
            "Running `bob gkeep` with no command runs `bob gkeep list`.\n\nExamples:\n  bob gkeep                    Show both inboxes and what `pull` would do\n  bob gkeep list -s vault      Show only gkeep_inbox.md tasks (no network)\n  bob gkeep pull -d            Preview the exact Markdown a pull would write\n  bob gkeep pull -n            Write and verify tasks, but leave notes in Keep\n  bob gkeep pull               Write, verify, commit, then archive in Keep\n  bob gkeep pull -i 3f9c2e1    Pull one note by its REF from `bob gkeep list`\n  bob gkeep doctor             Diagnose credentials, adapter, and connectivity\n  bob gkeep login              One-time setup of the Keep master token\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gkeep config; defaults to ~/.config/bob/config.yml\n  BOB_GKEEP_ADAPTER  adapter executable replacing `uv run --script …`",
        )
        .disable_help_flag(true)
        .arg(all_arg())
        .arg(bob_dir_arg())
        .arg(list_format_arg())
        .arg(help_arg())
        .arg(source_arg())
        .subcommand(doctor_command())
        .subcommand(list_command())
        .subcommand(login_command())
        .subcommand(migrate_markers_command())
        .subcommand(migrate_tasks_command())
        .subcommand(pull_command())
}

fn doctor_command() -> ClapCommand {
    ClapCommand::new("doctor")
        .about(
            "Check the Keep setup: config, token, adapter, Keep, and \
             target note",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(human_format_arg())
        .arg(help_arg())
        .after_help(doctor_after_help())
}

fn doctor_after_help() -> &'static str {
    "Examples:\n  bob gkeep doctor\n  bob gkeep doctor -f json\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gkeep config; defaults to ~/.config/bob/config.yml\n  BOB_GKEEP_ADAPTER  adapter executable replacing `uv run --script …`"
}

fn list_command() -> ClapCommand {
    ClapCommand::new("list")
        .about(
            "Show Keep inbox notes and gkeep_inbox.md tasks side by side \
             (default)",
        )
        .disable_help_flag(true)
        .arg(all_arg())
        .arg(bob_dir_arg())
        .arg(list_format_arg())
        .arg(help_arg())
        .arg(source_arg())
        .after_help(list_after_help())
}

fn list_after_help() -> &'static str {
    "Examples:\n  bob gkeep\n  bob gkeep list\n  bob gkeep list -s vault\n  bob gkeep list -a -f json\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gkeep config; defaults to ~/.config/bob/config.yml\n  BOB_GKEEP_ADAPTER  adapter executable replacing `uv run --script …`"
}

fn login_command() -> ClapCommand {
    ClapCommand::new("login")
        .about(
            "Exchange a Google sign-in cookie for a stored Keep master \
             token",
        )
        .long_about(
            "Exchange a Google sign-in cookie for a stored Keep master \
             token.\n\n\
             The cookie is read from a hidden TTY prompt, or from stdin \
             when stdin is not a TTY. It is exchanged through the adapter \
             and stored via `token_store_command`; the token is then read \
             back and checked for reachability.",
        )
        .disable_help_flag(true)
        .arg(email_arg())
        .arg(help_arg())
        .after_help(login_after_help())
}

fn login_after_help() -> &'static str {
    "Examples:\n  bob gkeep login\n  bob gkeep login -e bryanbugyi34@gmail.com\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gkeep config; defaults to ~/.config/bob/config.yml\n  BOB_GKEEP_ADAPTER  adapter executable replacing `uv run --script …`"
}

fn migrate_markers_command() -> ClapCommand {
    ClapCommand::new("migrate-markers")
        .about("Remove Google Keep bookkeeping markers from task Markdown (offline)")
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(human_format_arg())
        .arg(help_arg())
        .arg(no_commit_arg())
        .arg(quiet_arg())
        .after_help(migrate_markers_after_help())
}

fn migrate_markers_after_help() -> &'static str {
    "Offline migration: removes supported `%%gkeep:…%%` markers from task Markdown while preserving import evidence in `.bob/gkeep/imports/`. Needs no credentials, adapter, Keep snapshot, or network access, and does not require a configured target note.\n\nExamples:\n  bob gkeep migrate-markers --dry-run\n  bob gkeep migrate-markers\n  bob gkeep migrate-markers -d -f json\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob"
}

fn migrate_tasks_command() -> ClapCommand {
    ClapCommand::new("migrate-tasks")
        .about("Backfill compact Keep links on open tasks (offline)")
        .long_about(
            "Convert generated Google Keep Source children to the compact linked 💡 task format. This repair operation only considers open tasks; closed tasks remain unchanged.",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(human_format_arg())
        .arg(help_arg())
        .arg(no_commit_arg())
        .arg(quiet_arg())
        .after_help(migrate_tasks_after_help())
}

fn migrate_tasks_after_help() -> &'static str {
    "Offline migration: converts recognized generated Keep Source children on open tasks to one `[💡](URL \"Open in Google Keep\")` link, removes redundant timestamps and supported markers, and keeps labels, task metadata, children, and import history. This command repairs open tasks only. Ambiguous or unsupported content is left for review and reported. No credentials, adapter, configuration, snapshot, or network access is used.\n\nExamples:\n  bob gkeep migrate-tasks --dry-run --format json\n  bob gkeep migrate-tasks --dry-run\n  bob gkeep migrate-tasks\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to BOB_DIR or ~/bob"
}

fn pull_command() -> ClapCommand {
    ClapCommand::new("pull")
        .about(
            "Move Keep inbox notes into gkeep_inbox.md, then archive them \
             in Keep",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(human_format_arg())
        .arg(help_arg())
        .arg(id_arg())
        .arg(include_pinned_arg())
        .arg(include_shared_arg())
        .arg(limit_arg())
        .arg(no_archive_arg())
        .arg(no_commit_arg())
        .arg(no_ref_arg())
        .arg(parent_arg())
        .arg(quiet_arg())
        .after_help(pull_after_help())
}

fn pull_after_help() -> &'static str {
    "A note is archived only after its current content is verifiably in the vault: written atomically, fsynced, re-read and parsed, and committed when the vault is a Git worktree. Notes edited in Keep during a pull stay in Keep; the next pull writes the revision. Nothing is ever deleted from Keep. URL-only notes are clipped into the reading queue under a trailing @route, -P/--parent, a TTY prompt, or gkeep_inbox; -R keeps them as tasks instead.\n\nExamples:\n  bob gkeep pull --dry-run\n  bob gkeep pull\n  bob gkeep pull -d\n  bob gkeep pull -n\n  bob gkeep pull -i 3f9c2e1\n  bob gkeep pull -f json\n  bob gkeep pull -R\n  bob gkeep pull -P sase\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gkeep config; defaults to ~/.config/bob/config.yml\n  BOB_GKEEP_ADAPTER  adapter executable replacing `uv run --script …`"
}

fn all_arg() -> Arg {
    Arg::new("all")
        .long("all")
        .short('a')
        .action(ArgAction::SetTrue)
        .help("Also show archived Keep notes and done/canceled vault tasks")
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
        .help("Preview the exact Markdown a pull would write")
}

fn human_format_arg() -> Arg {
    Arg::new("format")
        .long("format")
        .short('f')
        .value_name("FORMAT")
        .value_parser(["human", "json"])
        .default_value("human")
        .help("Output format: human or json")
}

fn list_format_arg() -> Arg {
    Arg::new("format")
        .long("format")
        .short('f')
        .value_name("FORMAT")
        .value_parser(["table", "json"])
        .default_value("table")
        .help("Output format: table or json")
}

fn help_arg() -> Arg {
    Arg::new("help")
        .long("help")
        .short('h')
        .action(ArgAction::Help)
        .help("Show help")
}

fn id_arg() -> Arg {
    Arg::new("id")
        .long("id")
        .short('i')
        .value_name("REF")
        .action(ArgAction::Append)
        .value_parser(OsStringValueParser::new())
        .help(
            "Pull one note by its REF from `bob gkeep list` (or a full \
             Keep id); repeatable",
        )
}

fn include_pinned_arg() -> Arg {
    Arg::new("include-pinned")
        .long("include-pinned")
        .short('p')
        .action(ArgAction::SetTrue)
        .help("Include pinned notes instead of skipping them")
}

fn include_shared_arg() -> Arg {
    Arg::new("include-shared")
        .long("include-shared")
        .short('S')
        .action(ArgAction::SetTrue)
        .help("Include shared notes instead of skipping them")
}

fn limit_arg() -> Arg {
    Arg::new("limit")
        .long("limit")
        .short('l')
        .value_name("N")
        .value_parser(clap::value_parser!(u64))
        .help("Pull only the first N actionable notes, oldest first")
}

fn no_archive_arg() -> Arg {
    Arg::new("no-archive")
        .long("no-archive")
        .short('n')
        .action(ArgAction::SetTrue)
        .help("Write and verify tasks, but leave notes in Keep")
}

fn no_commit_arg() -> Arg {
    Arg::new("no-commit")
        .long("no-commit")
        .short('C')
        .action(ArgAction::SetTrue)
        .help("Skip the vault Git commit after writing")
}

fn no_ref_arg() -> Arg {
    Arg::new("no-ref")
        .long("no-ref")
        .short('R')
        .action(ArgAction::SetTrue)
        .help("Keep URL-only notes as inbox tasks instead of clipping them")
}

fn parent_arg() -> Arg {
    Arg::new("parent")
        .long("parent")
        .short('P')
        .value_name("ROUTE")
        .value_parser(OsStringValueParser::new())
        .help("Parent route for URL-only notes clipped into the reading queue")
}

fn quiet_arg() -> Arg {
    Arg::new("quiet")
        .long("quiet")
        .short('q')
        .action(ArgAction::SetTrue)
        .help("Print only errors")
}

fn email_arg() -> Arg {
    Arg::new("email")
        .long("email")
        .short('e')
        .value_name("EMAIL")
        .value_parser(OsStringValueParser::new())
        .help("Keep account email; overrides gkeep.email")
}

fn source_arg() -> Arg {
    Arg::new("source")
        .long("source")
        .short('s')
        .value_name("SOURCE")
        .value_parser(["both", "keep", "vault"])
        .default_value("both")
        .help("Show both inboxes, Keep only, or the vault only")
}

/// `list` output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListFormat {
    Table,
    Json,
}

impl ListFormat {
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

    /// The format name for error reporting (`table` counts as human).
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::Json => "json",
        }
    }

    /// Whether failures print the JSON error document.
    pub(crate) fn is_json(self) -> bool {
        matches!(self, Self::Json)
    }
}

/// `doctor`/`pull` output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HumanFormat {
    Human,
    Json,
}

impl HumanFormat {
    fn from_matches(matches: &ArgMatches) -> Self {
        match matches
            .get_one::<String>("format")
            .map(String::as_str)
            .unwrap_or("human")
        {
            "json" => Self::Json,
            _ => Self::Human,
        }
    }

    /// The format name for error reporting.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Json => "json",
        }
    }

    /// Whether failures print the JSON error document.
    pub(crate) fn is_json(self) -> bool {
        matches!(self, Self::Json)
    }
}

/// Which inbox `list` shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListSource {
    Both,
    Keep,
    Vault,
}

impl ListSource {
    fn from_matches(matches: &ArgMatches) -> Self {
        match matches
            .get_one::<String>("source")
            .map(String::as_str)
            .unwrap_or("both")
        {
            "keep" => Self::Keep,
            "vault" => Self::Vault,
            _ => Self::Both,
        }
    }
}

/// Typed `list` arguments (also carried by top-level `bob gkeep`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListArgs {
    /// Also archived Keep notes and done/canceled vault tasks.
    pub all: bool,
    /// Vault root override before `BOB_DIR` fallback.
    pub bob_dir: Option<PathBuf>,
    /// Output format.
    pub format: ListFormat,
    /// Which inbox to show.
    pub source: ListSource,
}

impl ListArgs {
    pub(crate) fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            all: matches.get_flag("all"),
            bob_dir: raw_bob_dir(matches),
            format: ListFormat::from_matches(matches),
            source: ListSource::from_matches(matches),
        }
    }

    /// The vault root: `--bob-dir`, else `BOB_DIR` or `~/bob`.
    pub(crate) fn bob_dir(&self) -> PathBuf {
        self.bob_dir.clone().unwrap_or_else(bob_env::bob_dir)
    }

    /// The error-reporting format name.
    pub(crate) fn error_format(&self) -> &'static str {
        self.format.as_str()
    }
}

/// Typed `pull` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PullArgs {
    /// Vault root override before `BOB_DIR` fallback.
    pub bob_dir: Option<PathBuf>,
    /// Preview the exact Markdown without writing.
    pub dry_run: bool,
    /// Output format.
    pub format: HumanFormat,
    /// Selected notes: full Keep ids or REF prefixes; repeatable.
    pub id: Vec<String>,
    /// Include pinned notes instead of skipping them.
    pub include_pinned: bool,
    /// Include shared notes instead of skipping them.
    pub include_shared: bool,
    /// Pull only the first N actionable notes, oldest first.
    pub limit: Option<u64>,
    /// Write and verify tasks, but leave notes in Keep.
    pub no_archive: bool,
    /// Skip the vault Git commit after writing.
    pub no_commit: bool,
    /// Keep URL-only notes as inbox tasks instead of clipping them.
    pub no_ref: bool,
    /// Parent route for URL-only notes clipped into the reading queue.
    pub parent: Option<String>,
    /// Print only errors.
    pub quiet: bool,
}

impl PullArgs {
    pub(crate) fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            bob_dir: raw_bob_dir(matches),
            dry_run: matches.get_flag("dry-run"),
            format: HumanFormat::from_matches(matches),
            id: matches
                .get_many::<OsString>("id")
                .map(|values| {
                    values
                        .map(|value| value.to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default(),
            include_pinned: matches.get_flag("include-pinned"),
            include_shared: matches.get_flag("include-shared"),
            limit: matches.get_one::<u64>("limit").copied(),
            no_archive: matches.get_flag("no-archive"),
            no_commit: matches.get_flag("no-commit"),
            no_ref: matches.get_flag("no-ref"),
            parent: matches
                .get_one::<OsString>("parent")
                .map(|value| value.to_string_lossy().into_owned()),
            quiet: matches.get_flag("quiet"),
        }
    }

    /// The vault root: `--bob-dir`, else `BOB_DIR` or `~/bob`.
    pub(crate) fn bob_dir(&self) -> PathBuf {
        self.bob_dir.clone().unwrap_or_else(bob_env::bob_dir)
    }

    /// The error-reporting format name.
    pub(crate) fn error_format(&self) -> &'static str {
        self.format.as_str()
    }
}

/// Typed `migrate-markers` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MigrateArgs {
    /// Vault root override before `BOB_DIR` fallback.
    pub bob_dir: Option<PathBuf>,
    /// Report exact proposed removals with zero writes.
    pub dry_run: bool,
    /// Output format.
    pub format: HumanFormat,
    /// Skip the vault Git commit after migrating.
    pub no_commit: bool,
    /// Print only errors.
    pub quiet: bool,
}

/// Typed `migrate-tasks` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MigrateTasksArgs {
    /// Vault root override before `BOB_DIR` fallback.
    pub bob_dir: Option<PathBuf>,
    /// Report exact proposed edits with zero writes.
    pub dry_run: bool,
    /// Output format.
    pub format: HumanFormat,
    /// Skip the vault Git commit after migrating.
    pub no_commit: bool,
    /// Print only errors.
    pub quiet: bool,
}

impl MigrateTasksArgs {
    pub(crate) fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            bob_dir: raw_bob_dir(matches),
            dry_run: matches.get_flag("dry-run"),
            format: HumanFormat::from_matches(matches),
            no_commit: matches.get_flag("no-commit"),
            quiet: matches.get_flag("quiet"),
        }
    }

    /// The vault root: `--bob-dir`, else `BOB_DIR` or `~/bob`.
    pub(crate) fn bob_dir(&self) -> PathBuf {
        self.bob_dir.clone().unwrap_or_else(bob_env::bob_dir)
    }

    /// The error-reporting format name.
    pub(crate) fn error_format(&self) -> &'static str {
        self.format.as_str()
    }
}

impl MigrateArgs {
    pub(crate) fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            bob_dir: raw_bob_dir(matches),
            dry_run: matches.get_flag("dry-run"),
            format: HumanFormat::from_matches(matches),
            no_commit: matches.get_flag("no-commit"),
            quiet: matches.get_flag("quiet"),
        }
    }

    /// The vault root: `--bob-dir`, else `BOB_DIR` or `~/bob`.
    pub(crate) fn bob_dir(&self) -> PathBuf {
        self.bob_dir.clone().unwrap_or_else(bob_env::bob_dir)
    }

    /// The error-reporting format name.
    pub(crate) fn error_format(&self) -> &'static str {
        self.format.as_str()
    }
}

/// Typed `doctor` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DoctorArgs {
    /// Vault root override before `BOB_DIR` fallback.
    pub bob_dir: Option<PathBuf>,
    /// Output format.
    pub format: HumanFormat,
}

impl DoctorArgs {
    pub(crate) fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            bob_dir: raw_bob_dir(matches),
            format: HumanFormat::from_matches(matches),
        }
    }

    /// The vault root: `--bob-dir`, else `BOB_DIR` or `~/bob`.
    pub(crate) fn bob_dir(&self) -> PathBuf {
        self.bob_dir.clone().unwrap_or_else(bob_env::bob_dir)
    }
}

/// Typed `login` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoginArgs {
    /// Keep account email; overrides `gkeep.email`.
    pub email: Option<String>,
}

impl LoginArgs {
    pub(crate) fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            email: matches
                .get_one::<OsString>("email")
                .map(|value| value.to_string_lossy().into_owned()),
        }
    }
}

fn raw_bob_dir(matches: &ArgMatches) -> Option<PathBuf> {
    matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches_for(args: &[&str]) -> ArgMatches {
        let command = build_cli();
        let owned: Vec<OsString> = args.iter().map(OsString::from).collect();
        command.try_get_matches_from(owned).expect("args parse")
    }

    fn sub_matches_for(args: &[&str], subcommand: &str) -> ArgMatches {
        let matches = matches_for(args);
        let (_, sub_matches) = matches
            .subcommand()
            .filter(|(name, _)| *name == subcommand)
            .expect("subcommand matches");
        sub_matches.clone()
    }

    #[test]
    fn list_args_pin_defaults_and_values() {
        let args = ListArgs::from_matches(&matches_for(&["bob gkeep"]));
        assert!(!args.all);
        assert_eq!(args.bob_dir(), bob_env::bob_dir());
        assert_eq!(args.format, ListFormat::Table);
        assert_eq!(args.source, ListSource::Both);
        assert_eq!(args.error_format(), "table");
        assert!(!args.format.is_json());

        let args = ListArgs::from_matches(&sub_matches_for(
            &["bob gkeep", "list", "-a", "-f", "json", "-s", "vault"],
            "list",
        ));
        assert!(args.all);
        assert_eq!(args.format, ListFormat::Json);
        assert!(args.format.is_json());
        assert_eq!(args.source, ListSource::Vault);
        assert_eq!(args.error_format(), "json");
    }

    #[test]
    fn pull_args_pin_every_option() {
        let args = PullArgs::from_matches(&sub_matches_for(
            &[
                "bob gkeep",
                "pull",
                "-d",
                "-f",
                "json",
                "-i",
                "3f9c2e1",
                "-i",
                "full-id",
                "-p",
                "-S",
                "-l",
                "5",
                "-n",
                "-C",
                "-R",
                "-q",
            ],
            "pull",
        ));
        assert!(args.dry_run);
        assert_eq!(args.format, HumanFormat::Json);
        assert!(args.format.is_json());
        assert_eq!(args.id, vec!["3f9c2e1", "full-id"]);
        assert!(args.include_pinned);
        assert!(args.include_shared);
        assert_eq!(args.limit, Some(5));
        assert!(args.no_archive);
        assert!(args.no_commit);
        assert!(args.no_ref);
        assert!(args.quiet);
        assert_eq!(args.error_format(), "json");

        let args = PullArgs::from_matches(&sub_matches_for(
            &["bob gkeep", "pull"],
            "pull",
        ));
        assert_eq!(args.id, Vec::<String>::new());
        assert_eq!(args.limit, None);
        assert!(!args.no_ref);
        assert_eq!(args.format, HumanFormat::Human);
        assert_eq!(args.error_format(), "human");
    }

    #[test]
    fn doctor_and_login_args() {
        let args = DoctorArgs::from_matches(&sub_matches_for(
            &["bob gkeep", "doctor", "-f", "json"],
            "doctor",
        ));
        assert_eq!(args.format, HumanFormat::Json);
        assert!(args.format.is_json());
        assert_eq!(args.bob_dir(), bob_env::bob_dir());

        let args = LoginArgs::from_matches(&sub_matches_for(
            &["bob gkeep", "login", "-e", "a@b.c"],
            "login",
        ));
        assert_eq!(args.email.as_deref(), Some("a@b.c"));

        let args = LoginArgs::from_matches(&sub_matches_for(
            &["bob gkeep", "login"],
            "login",
        ));
        assert_eq!(args.email, None);
    }

    #[test]
    fn migrate_tasks_args_pin_options() {
        let args = MigrateTasksArgs::from_matches(&sub_matches_for(
            &[
                "bob gkeep",
                "migrate-tasks",
                "-b",
                "~/vault",
                "-d",
                "-f",
                "json",
                "-C",
                "-q",
            ],
            "migrate-tasks",
        ));
        assert_eq!(args.bob_dir(), bob_env::home_dir().join("vault"));
        assert!(args.dry_run);
        assert_eq!(args.format, HumanFormat::Json);
        assert!(args.no_commit);
        assert!(args.quiet);
    }

    #[test]
    fn bob_dir_flag_expands_tilde() {
        let home = bob_env::home_dir();
        let args = ListArgs::from_matches(&sub_matches_for(
            &["bob gkeep", "list", "-b", "~/vault"],
            "list",
        ));
        assert_eq!(args.bob_dir(), home.join("vault"));
    }
}

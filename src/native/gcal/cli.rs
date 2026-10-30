//! `bob gcal` command tree, help text, and typed arguments.

use std::{ffi::OsString, path::PathBuf};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};

use crate::native::env as bob_env;

const COMMAND_NAME: &str = "bob gcal";

/// The full `bob gcal` command tree.
pub(crate) fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Project Google Calendar events onto today's daily note")
        .long_about(
            "Google Calendar → daily-note Event Tasks: one read-only login \
             through gog, then list and sync (later) write accepted events \
             above today's Pomodoros. This phase ships calendars, doctor, \
             login, and logout.",
        )
        .after_help(top_after_help())
        .disable_help_flag(true)
        .arg(help_arg())
        .subcommand(calendars_command())
        .subcommand(doctor_command())
        .subcommand(login_command())
        .subcommand(logout_command())
}

fn top_after_help() -> &'static str {
    "Running `bob gcal` with no command prints this help. `list` and \
     `sync` arrive in a later release.\n\nExamples:\n  bob gcal login              One-time read-only Calendar consent\n  bob gcal login -c PATH      Store a Desktop-app client JSON first\n  bob gcal login -m           Browserless sign-in (paste the redirect URL)\n  bob gcal calendars          List calendars with ids for config\n  bob gcal doctor             Diagnose gog, the client, and the vault\n  bob gcal logout -y          Remove Bob's Calendar grant only\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gcal config; defaults to ~/.config/bob/config.yml\n  BOB_GCAL_GOG       gog executable replacing `gog` on PATH"
}

fn calendars_command() -> ClapCommand {
    ClapCommand::new("calendars")
        .about(
            "List Google calendars with id, access role, colour, and mapped \
             root",
        )
        .disable_help_flag(true)
        .arg(list_format_arg())
        .arg(help_arg())
        .after_help(calendars_after_help())
}

fn calendars_after_help() -> &'static str {
    "Examples:\n  bob gcal calendars\n  bob gcal calendars -f json\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gcal config; defaults to ~/.config/bob/config.yml\n  BOB_GCAL_GOG       gog executable replacing `gog` on PATH"
}

fn doctor_command() -> ClapCommand {
    ClapCommand::new("doctor")
        .about(
            "Check the Calendar setup: gog, client, account, token, \
             calendars, default root, today, journal, and writer",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(human_format_arg())
        .arg(help_arg())
        .after_help(doctor_after_help())
}

fn doctor_after_help() -> &'static str {
    "Examples:\n  bob gcal doctor\n  bob gcal doctor -f json\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gcal config; defaults to ~/.config/bob/config.yml\n  BOB_GCAL_GOG       gog executable replacing `gog` on PATH"
}

fn login_command() -> ClapCommand {
    ClapCommand::new("login")
        .about("One-time read-only Google Calendar consent through gog")
        .long_about(
            "One-time read-only Google Calendar consent through gog.\n\n\
             Uses an isolated `--client bob-gcal` token bucket so removing \
             or re-consenting Bob's grant never touches other gog tokens. \
             `-c FILE` stores a Desktop-app OAuth client first. `-m` runs \
             the browserless paste-URL flow, which is the right choice \
             over SSH.",
        )
        .disable_help_flag(true)
        .arg(account_arg())
        .arg(credentials_arg())
        .arg(human_format_arg())
        .arg(help_arg())
        .arg(manual_arg())
        .after_help(login_after_help())
}

fn login_after_help() -> &'static str {
    "Examples:\n  bob gcal login\n  bob gcal login -a you@example.com\n  bob gcal login -c ~/Downloads/client_secret.json\n  bob gcal login -m\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gcal config; defaults to ~/.config/bob/config.yml\n  BOB_GCAL_GOG       gog executable replacing `gog` on PATH"
}

fn logout_command() -> ClapCommand {
    ClapCommand::new("logout")
        .about("Remove Bob's Calendar grant only; prints the Google revoke URL")
        .disable_help_flag(true)
        .arg(account_arg())
        .arg(force_arg())
        .arg(human_format_arg())
        .arg(help_arg())
        .after_help(logout_after_help())
}

fn logout_after_help() -> &'static str {
    "Removes the isolated `bob-gcal` refresh token. Gmail and other gog \
     clients are left alone. Then revoke Bob's Calendar access at \
     https://myaccount.google.com/permissions.\n\nExamples:\n  bob gcal logout -y\n  bob gcal logout -a you@example.com -y\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gcal config; defaults to ~/.config/bob/config.yml\n  BOB_GCAL_GOG       gog executable replacing `gog` on PATH"
}

fn account_arg() -> Arg {
    Arg::new("account")
        .long("account")
        .short('a')
        .value_name("EMAIL")
        .value_parser(OsStringValueParser::new())
        .help("Google account email; overrides gcal.account")
}

fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .long("bob-dir")
        .short('b')
        .value_name("DIR")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

fn credentials_arg() -> Arg {
    Arg::new("credentials")
        .long("credentials")
        .short('c')
        .value_name("FILE")
        .value_parser(OsStringValueParser::new())
        .help("Desktop-app OAuth client JSON to store as bob-gcal")
}

fn force_arg() -> Arg {
    Arg::new("force")
        .long("force")
        .short('y')
        .action(ArgAction::SetTrue)
        .help("Skip the logout confirmation")
}

fn help_arg() -> Arg {
    Arg::new("help")
        .long("help")
        .short('h')
        .action(ArgAction::Help)
        .help("Show help")
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

fn manual_arg() -> Arg {
    Arg::new("manual")
        .long("manual")
        .short('m')
        .action(ArgAction::SetTrue)
        .help("Browserless sign-in: paste the redirect URL")
}

/// `doctor`/`login`/`logout` output format.
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

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Json => "json",
        }
    }

    pub(crate) fn is_json(self) -> bool {
        matches!(self, Self::Json)
    }
}

/// `calendars` output format.
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

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::Json => "json",
        }
    }

    pub(crate) fn is_json(self) -> bool {
        matches!(self, Self::Json)
    }
}

/// Typed `calendars` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CalendarsArgs {
    pub format: ListFormat,
}

impl CalendarsArgs {
    pub(crate) fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            format: ListFormat::from_matches(matches),
        }
    }

    pub(crate) fn error_format(&self) -> &'static str {
        self.format.as_str()
    }
}

/// Typed `doctor` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DoctorArgs {
    pub bob_dir: Option<PathBuf>,
    pub format: HumanFormat,
}

impl DoctorArgs {
    pub(crate) fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            bob_dir: raw_bob_dir(matches),
            format: HumanFormat::from_matches(matches),
        }
    }

    pub(crate) fn bob_dir(&self) -> PathBuf {
        self.bob_dir.clone().unwrap_or_else(bob_env::bob_dir)
    }
}

/// Typed `login` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoginArgs {
    pub account: Option<String>,
    pub credentials: Option<PathBuf>,
    pub format: HumanFormat,
    pub manual: bool,
}

impl LoginArgs {
    pub(crate) fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            account: matches
                .get_one::<OsString>("account")
                .map(|value| value.to_string_lossy().trim().to_string()),
            credentials: matches
                .get_one::<OsString>("credentials")
                .map(PathBuf::from)
                .map(|path| bob_env::expand_tilde(&path)),
            format: HumanFormat::from_matches(matches),
            manual: matches.get_flag("manual"),
        }
    }
}

/// Typed `logout` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LogoutArgs {
    pub account: Option<String>,
    pub format: HumanFormat,
    pub force: bool,
}

impl LogoutArgs {
    pub(crate) fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            account: matches
                .get_one::<OsString>("account")
                .map(|value| value.to_string_lossy().trim().to_string()),
            format: HumanFormat::from_matches(matches),
            force: matches.get_flag("force"),
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
    fn login_args_pin_options() {
        let args = LoginArgs::from_matches(&sub_matches_for(
            &[
                "bob gcal",
                "login",
                "-a",
                "you@example.com",
                "-c",
                "~/client.json",
                "-f",
                "json",
                "-m",
            ],
            "login",
        ));
        assert_eq!(args.account.as_deref(), Some("you@example.com"));
        assert_eq!(
            args.credentials.as_deref(),
            Some(bob_env::home_dir().join("client.json").as_path())
        );
        assert!(args.format.is_json());
        assert!(args.manual);
    }

    #[test]
    fn logout_and_doctor_args() {
        let args = LogoutArgs::from_matches(&sub_matches_for(
            &["bob gcal", "logout", "-a", "you@example.com", "-y"],
            "logout",
        ));
        assert_eq!(args.account.as_deref(), Some("you@example.com"));
        assert!(args.force);

        let args = DoctorArgs::from_matches(&sub_matches_for(
            &["bob gcal", "doctor", "-b", "~/vault", "-f", "json"],
            "doctor",
        ));
        assert_eq!(args.bob_dir(), bob_env::home_dir().join("vault"));
        assert!(args.format.is_json());
    }

    #[test]
    fn calendars_format_defaults_to_table() {
        let args = CalendarsArgs::from_matches(&sub_matches_for(
            &["bob gcal", "calendars"],
            "calendars",
        ));
        assert_eq!(args.format, ListFormat::Table);
        assert_eq!(args.error_format(), "table");
    }
}

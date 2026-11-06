//! `bob gmail` command tree, help text, and typed arguments.

use std::{ffi::OsString, path::PathBuf};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};

use crate::native::env as bob_env;

const COMMAND_NAME: &str = "bob gmail";

/// The full `bob gmail` command tree.
pub(crate) fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Save Gmail threads as inert mail notes, linked from tasks")
        .long_about(
            "Gmail threads you choose become inert, append-only mail notes: \
             one read-only login through gog, then save threads offline and \
             link them from tasks. Every send goes through a previewed Gmail \
             draft. Bare `bob gmail` lists saved threads.",
        )
        .after_help(top_after_help())
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(list_format_arg())
        .arg(help_arg())
        .subcommand(doctor_command())
        .subcommand(login_command())
        .subcommand(logout_command())
}

fn top_after_help() -> &'static str {
    "Running `bob gmail` with no command lists saved threads (offline, read-only).\n\nExamples:\n  bob gmail                   List saved threads\n  bob gmail -f json           List saved threads as JSON\n  bob gmail login             Connect Gmail in one guided step\n  bob gmail login -a EMAIL    Sign in as this account\n  bob gmail login -s compose  Also grant the compose scope (Mac only)\n  bob gmail doctor            Diagnose gog, the client, and the vault\n  bob gmail logout -y         Remove Bob's Gmail grant only\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gmail config; defaults to ~/.config/bob/config.yml\n  BOB_GMAIL_GOG      gog executable replacing `gog` on PATH\n  BOB_GMAIL_BROWSER  Browser executable; selects browser sign-in (otherwise automatic)"
}

fn doctor_command() -> ClapCommand {
    ClapCommand::new("doctor")
        .about(
            "Check the Gmail setup: gog, client, account, token, scopes, \
             config, writer, and journal",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(human_format_arg())
        .arg(help_arg())
        .after_help(doctor_after_help())
}

fn doctor_after_help() -> &'static str {
    "Examples:\n  bob gmail doctor\n  bob gmail doctor -f json\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gmail config; defaults to ~/.config/bob/config.yml\n  BOB_GMAIL_GOG      gog executable replacing `gog` on PATH"
}

fn login_command() -> ClapCommand {
    ClapCommand::new("login")
        .about("Connect Gmail (read-only) in one guided step")
        .long_about(
            "Connect Gmail in one guided step.\n\n\
             Uses an isolated `--client bob-gmail` token bucket so removing \
             or re-consenting Bob's grant never touches other gog tokens. \
             Bob reuses a readable gog client or checks ~/Downloads before \
             guiding you through client setup. Sign-in opens a browser when \
             a desktop session is available and uses the paste-URL flow \
             automatically over SSH. Rerunning a healthy login is \
             idempotent: it reports already connected and rechecks Gmail \
             access. `-c FILE` replaces Bob's stored client. `-m` forces \
             paste sign-in. `-s compose` also requests the Gmail compose \
             scope (which also permits sending); the gate is the \
             preview/confirm step plus keeping that grant off agent hosts.",
        )
        .disable_help_flag(true)
        .arg(account_arg())
        .arg(credentials_arg())
        .arg(human_format_arg())
        .arg(help_arg())
        .arg(manual_arg())
        .arg(scope_arg())
        .after_help(login_after_help())
}

fn login_after_help() -> &'static str {
    "Examples:\n  bob gmail login\n  bob gmail login -a you@example.com\n  bob gmail login -c ~/Downloads/client_secret.json\n  bob gmail login -m\n  bob gmail login -s compose\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gmail config; defaults to ~/.config/bob/config.yml\n  BOB_GMAIL_GOG      gog executable replacing `gog` on PATH\n  BOB_GMAIL_BROWSER  Browser executable; selects browser sign-in (otherwise automatic)"
}

fn logout_command() -> ClapCommand {
    ClapCommand::new("logout")
        .about("Remove Bob's Gmail grant only; prints the Google revoke URL")
        .disable_help_flag(true)
        .arg(account_arg())
        .arg(force_arg())
        .arg(human_format_arg())
        .arg(help_arg())
        .after_help(logout_after_help())
}

fn logout_after_help() -> &'static str {
    "Removes the isolated `bob-gmail` refresh token. Calendar, Keep, and other gog \
     clients are left alone. Then revoke Bob's Gmail access at \
     https://myaccount.google.com/permissions.\n\nExamples:\n  bob gmail logout -y\n  bob gmail logout -a you@example.com -y\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gmail config; defaults to ~/.config/bob/config.yml\n  BOB_GMAIL_GOG      gog executable replacing `gog` on PATH"
}

fn account_arg() -> Arg {
    Arg::new("account")
        .long("account")
        .short('a')
        .value_name("EMAIL")
        .value_parser(OsStringValueParser::new())
        .help("Google account email; overrides gmail.account")
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
        .help("Desktop-app client JSON to store (replaces Bob's stored client)")
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
        .help("Sign in by pasting the redirect address (automatic over SSH)")
}

fn scope_arg() -> Arg {
    Arg::new("scope")
        .long("scope")
        .short('s')
        .value_name("SCOPE")
        .value_parser(["readonly", "compose"])
        .default_value("readonly")
        .help("OAuth scope: readonly, or compose (also permits sending)")
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

/// Bare `bob gmail` output format.
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

    // Consumed by gmail-save's list; shell bare only needs `is_json`.
    #[allow(dead_code)]
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

/// OAuth scope requested by `login`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoginScope {
    Readonly,
    Compose,
}

impl LoginScope {
    fn from_matches(matches: &ArgMatches) -> Self {
        match matches
            .get_one::<String>("scope")
            .map(String::as_str)
            .unwrap_or("readonly")
        {
            "compose" => Self::Compose,
            _ => Self::Readonly,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Readonly => "readonly",
            Self::Compose => "compose",
        }
    }
}

/// Typed bare `bob gmail` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TopArgs {
    pub bob_dir: Option<PathBuf>,
    pub format: ListFormat,
}

impl TopArgs {
    pub(crate) fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            bob_dir: raw_bob_dir(matches),
            format: ListFormat::from_matches(matches),
        }
    }

    // Consumed by gmail-save's list; shell bare resolves config only.
    #[allow(dead_code)]
    pub(crate) fn bob_dir(&self) -> PathBuf {
        self.bob_dir.clone().unwrap_or_else(bob_env::bob_dir)
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
    pub scope: LoginScope,
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
            scope: LoginScope::from_matches(matches),
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
                "bob gmail",
                "login",
                "-a",
                "you@example.com",
                "-c",
                "~/client.json",
                "-f",
                "json",
                "-m",
                "-s",
                "compose",
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
        assert_eq!(args.scope, LoginScope::Compose);
    }

    #[test]
    fn logout_and_doctor_args() {
        let args = LogoutArgs::from_matches(&sub_matches_for(
            &["bob gmail", "logout", "-a", "you@example.com", "-y"],
            "logout",
        ));
        assert_eq!(args.account.as_deref(), Some("you@example.com"));
        assert!(args.force);

        let args = DoctorArgs::from_matches(&sub_matches_for(
            &["bob gmail", "doctor", "-b", "~/vault", "-f", "json"],
            "doctor",
        ));
        assert_eq!(args.bob_dir(), bob_env::home_dir().join("vault"));
        assert!(args.format.is_json());
    }

    #[test]
    fn top_defaults_to_table() {
        let args = TopArgs::from_matches(&matches_for(&["bob gmail"]));
        assert_eq!(args.format, ListFormat::Table);
    }
}

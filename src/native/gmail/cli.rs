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
             draft. The `new` and `reply` commands create local notes without \
             sending. Bare `bob gmail` lists saved threads.",
        )
        .after_help(top_after_help())
        .disable_help_flag(true)
        .arg(all_arg())
        .arg(bob_dir_arg())
        .arg(list_format_arg())
        .arg(remote_arg())
        .arg(help_arg())
        .subcommand(attach_command())
        .subcommand(doctor_command())
        .subcommand(draft_command())
        .subcommand(link_command())
        .subcommand(list_command())
        .subcommand(login_command())
        .subcommand(logout_command())
        .subcommand(new_command())
        .subcommand(peek_command())
        .subcommand(reply_command())
        .subcommand(save_command())
        .subcommand(search_command())
        .subcommand(send_command())
        .subcommand(sync_command())
}

fn top_after_help() -> &'static str {
    "Running `bob gmail` with no command lists saved threads (offline, read-only). `-r` adds live message counts.\n\nExamples:\n  bob gmail                   List saved threads\n  bob gmail -a                List every saved thread\n  bob gmail -r                Fetch live new-message counts\n  bob gmail -f json           List saved threads as JSON\n  bob gmail new -t EMAIL      Create a local compose note\n  bob gmail peek URL          Preview a Gmail link\n  bob gmail reply NOTE        Add a Reply section\n  bob gmail send NOTE         Preview and confirm a Gmail draft send\n  bob gmail save URL          Save a Gmail thread\n  bob gmail search            Search recent inbox threads\n  bob gmail save ID -d        Preview a save without writing\n  bob gmail link tasks.md     Swap Gmail URLs for labelled links\n  bob gmail link -a -d        Preview a backfill of old URLs\n  bob gmail sync              File labelled threads into area or project notes\n  bob gmail sync -d           Preview the label sync\n  bob gmail attach NOTE 1     Import the first listed attachment\n  bob gmail draft NOTE        Create or update and preview a Gmail draft\n  bob gmail login             Connect Gmail in one guided step\n  bob gmail login -a EMAIL    Sign in as this account\n  bob gmail login -s compose  Also grant the compose scope (Mac only)\n  bob gmail doctor            Diagnose gog, the client, and the vault\n  bob gmail logout -y         Remove Bob's Gmail grant only\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gmail config; defaults to ~/.config/bob/config.yml\n  BOB_GMAIL_GOG      overrides gmail.gog and gog on PATH\n  BOB_GMAIL_BROWSER  Browser executable; selects browser sign-in (otherwise automatic)"
}
fn attach_command() -> ClapCommand {
    ClapCommand::new("attach")
        .about("Import one listed Gmail attachment into the vault")
        .long_about(
            "Download one attachment listed in a saved mail note and replace only its attachment row with a local file link. Gmail is read-only; attachment bytes are kept in memory until written to the vault. Repeating an import is a no-op.",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(save_format_arg())
        .arg(help_arg())
        .arg(
            Arg::new("note")
                .value_name("NOTE")
                .value_parser(OsStringValueParser::new())
                .required(true)
                .help("Path to a saved mail note"),
        )
        .arg(
            Arg::new("number")
                .value_name("N")
                .value_parser(clap::value_parser!(u32))
                .required(true)
                .help("1-based attachment number shown in the note"),
        )
        .after_help(
            "Examples:\n  bob gmail attach mail/2026/lease.md 1\n  bob gmail attach mail/2026/lease.md 2 -d -f json\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to BOB_DIR or ~/bob\n  BOB_CONFIG_FILE    Gmail config; defaults to ~/.config/bob/config.yml\n  BOB_GMAIL_GOG      overrides gmail.gog and gog on PATH",
        )
}

fn sync_command() -> ClapCommand {
    ClapCommand::new("sync")
        .alias("pull")
        .about("File labelled Gmail threads into area or project notes")
        .long_about(
            "Read the configured Gmail enrollment label (default: bob), save each new thread as a mail note, and append one task linking it from the area or project named by a routing label, or from the configured inbox file when none or an invalid routing label is present (default: gmail_inbox.md). Sync is read-only in Gmail and idempotent by thread identity. `pull` is a hidden alias.",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(save_format_arg())
        .arg(help_arg())
        .arg(sync_limit_arg())
        .arg(quiet_arg())
        .arg(retry_timeout_arg())
        .after_help(
            "Examples:\n  bob gmail sync\n  bob gmail sync -l 5\n  bob gmail sync -l 0\n  bob gmail sync -d -f json\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to BOB_DIR or ~/bob\n  BOB_CONFIG_FILE    Gmail config; defaults to ~/.config/bob/config.yml\n  BOB_GMAIL_GOG      overrides gmail.gog and gog on PATH",
        )
}

fn sync_limit_arg() -> Arg {
    Arg::new("limit")
        .long("limit")
        .short('l')
        .value_name("N")
        .value_parser(clap::value_parser!(u32))
        .default_value("50")
        .help("Maximum new filings (default 50; 0–500)")
}

fn retry_timeout_arg() -> Arg {
    Arg::new("retry-timeout")
        .long("retry-timeout")
        .short('r')
        .value_name("SECS")
        .value_parser(clap::value_parser!(u32))
        .default_value("120")
        .help("Shared retry budget in seconds (default 120)")
}

fn save_command() -> ClapCommand {
    ClapCommand::new("save")
        .about("Save or refresh Gmail threads as inert mail notes")
        .long_about(
            "Resolve Gmail URLs, thread or message IDs, RFC message IDs, and saved mail-note paths to confirmed threads. Gmail is read-only; writes stay in the configured vault. All network work finishes before the vault lock is acquired.",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(body_arg())
        .arg(dry_run_arg())
        .arg(save_format_arg())
        .arg(parent_arg())
        .arg(quiet_arg())
        .arg(rerender_arg())
        .arg(Arg::new("input")
            .value_name("URL|ID|NOTE")
            .value_parser(OsStringValueParser::new())
            .num_args(1..)
            .required(true)
            .help("Gmail URL, thread or message ID, RFC message ID, or saved mail note"))
        .arg(help_arg())
        .after_help(
            "Examples:\n  bob gmail save https://mail.google.com/mail/u/0/#all/0123456789abcdef\n  bob gmail save 0123456789abcdef -B headers\n  bob gmail save mail/2026/lease.md -P work\n  bob gmail save URL1 URL2 -d -f json\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gmail config; defaults to ~/.config/bob/config.yml\n  BOB_GMAIL_GOG      overrides gmail.gog and gog on PATH",
        )
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
    "Examples:\n  bob gmail doctor\n  bob gmail doctor -f json\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gmail config; defaults to ~/.config/bob/config.yml\n  BOB_GMAIL_GOG      overrides gmail.gog and gog on PATH"
}

fn draft_command() -> ClapCommand {
    ClapCommand::new("draft")
        .about("Create or update a Gmail draft and show its exact preview")
        .long_about(
            "Render the selected compose note, create or update its recorded Gmail draft, read the stored draft back, and show the canonical preview. This writes a Gmail draft but never sends it. Requires the isolated bob-gmail compose grant and the configured Gmail writer host.",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(human_format_arg())
        .arg(
            Arg::new("note")
                .value_name("NOTE")
                .value_parser(OsStringValueParser::new())
                .required(true)
                .help("Vault-relative compose or saved reply note"),
        )
        .arg(help_arg())
        .after_help(
            "Examples:\n  bob gmail draft mail/2026/261106_lease_renewal.md\n  bob gmail draft 261106_lease_renewal -f json\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to BOB_DIR or ~/bob\n  BOB_CONFIG_FILE    Gmail config; defaults to ~/.config/bob/config.yml\n  BOB_GMAIL_GOG      overrides gmail.gog and gog on PATH",
        )
}

fn send_command() -> ClapCommand {
    ClapCommand::new("send")
        .about("Preview and explicitly send a Gmail draft")
        .long_about(
            "Create or update the selected Gmail draft, read back its exact preview, and send only after confirmation bound to that preview. `--dry-run` writes the draft and shows the preview without sending. Requires the isolated bob-gmail compose grant and configured Gmail writer host.",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(
            Arg::new("confirm")
                .short('c')
                .long("confirm")
                .value_name("DIGEST")
                .help("Confirm the exact preview digest (required without a TTY)"),
        )
        .arg(dry_run_arg())
        .arg(human_format_arg())
        .arg(
            Arg::new("note")
                .value_name("NOTE")
                .value_parser(OsStringValueParser::new())
                .required(true)
                .help("Vault-relative compose or saved reply note"),
        )
        .arg(help_arg())
        .after_help(
            "Examples:\n  bob gmail send mail/2026/261106_lease_renewal.md -d\n  bob gmail send mail/2026/261106_lease_renewal.md\n  bob gmail send mail/2026/261106_lease_renewal.md -c 0123abcd...\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to BOB_DIR or ~/bob\n  BOB_CONFIG_FILE    Gmail config; defaults to ~/.config/bob/config.yml\n  BOB_GMAIL_GOG      overrides gmail.gog and gog on PATH",
        )
}

fn link_command() -> ClapCommand {
    ClapCommand::new("link")
        .about("Swap Gmail URLs for labelled links to saved mail notes")
        .long_about(
            "Find Gmail URLs in the named notes (or the whole vault with \
             `-a`), save each confirmed thread, and replace exactly those \
             URL tokens with labelled wikilinks. Dead links are remembered \
             so later sweeps skip them. Gmail is read-only; writes stay in \
             the configured vault. All network work finishes before the \
             vault lock is acquired.",
        )
        .disable_help_flag(true)
        .arg(all_arg())
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(save_format_arg())
        .arg(quiet_arg())
        .arg(Arg::new("note")
            .value_name("NOTE")
            .value_parser(OsStringValueParser::new())
            .num_args(1..)
            .help("Vault-relative mail note to scan for Gmail URLs"))
        .arg(help_arg())
        .after_help(
            "Examples:\n  bob gmail link tasks.md\n  bob gmail link one.md two.md -q\n  bob gmail link -a -d\n  bob gmail link -a -f json\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gmail config; defaults to ~/.config/bob/config.yml\n  BOB_GMAIL_GOG      overrides gmail.gog and gog on PATH",
        )
}

fn list_command() -> ClapCommand {
    ClapCommand::new("list")
        .about("List saved Gmail threads (offline unless --remote)")
        .disable_help_flag(true)
        .arg(all_arg())
        .arg(bob_dir_arg())
        .arg(list_format_arg())
        .arg(remote_arg())
        .arg(help_arg())
        .after_help(
            "Examples:\n  bob gmail list\n  bob gmail list -a\n  bob gmail list -r\n  bob gmail list -f json\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    Gmail config; defaults to ~/.config/bob/config.yml\n  BOB_GMAIL_GOG      overrides gmail.gog and gog on PATH",
        )
}

fn peek_command() -> ClapCommand {
    ClapCommand::new("peek")
        .about("Preview Gmail thread metadata without saving it")
        .long_about(
            "Resolve a Gmail URL or ID and return a small metadata-only preview. The result is cached privately for ten minutes; message bodies are never fetched or stored.",
        )
        .disable_help_flag(true)
        .arg(human_format_arg())
        .arg(Arg::new("input")
            .value_name("URL|ID")
            .value_parser(OsStringValueParser::new())
            .required(true)
            .help("Gmail URL or thread/message ID"))
        .arg(help_arg())
        .after_help(
            "Examples:\n  bob gmail peek https://mail.google.com/mail/u/0/#all/0123456789abcdef\n  bob gmail peek 0123456789abcdef -f json\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to BOB_DIR or ~/bob\n  BOB_CONFIG_FILE    Gmail config; defaults to ~/.config/bob/config.yml\n  BOB_GMAIL_GOG      overrides gmail.gog and gog on PATH",
        )
}

fn search_command() -> ClapCommand {
    ClapCommand::new("search")
        .about("Search Gmail threads for a picker")
        .long_about(
            "Search with Gmail query syntax. An empty query defaults to `in:inbox newer_than:30d`. Results are bounded to 50 threads and contain inert metadata only.",
        )
        .disable_help_flag(true)
        .arg(search_format_arg())
        .arg(limit_arg())
        .arg(Arg::new("query")
            .value_name("QUERY")
            .value_parser(OsStringValueParser::new())
            .help("Gmail query; defaults to recent inbox threads"))
        .arg(help_arg())
        .after_help(
            "Examples:\n  bob gmail search\n  bob gmail search from:ada@example.com -l 10\n  bob gmail search \"newer_than:7d has:attachment\" -f json\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to BOB_DIR or ~/bob\n  BOB_CONFIG_FILE    Gmail config; defaults to ~/.config/bob/config.yml\n  BOB_GMAIL_GOG      overrides gmail.gog and gog on PATH",
        )
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
    "Examples:\n  bob gmail login\n  bob gmail login -a you@example.com\n  bob gmail login -c ~/Downloads/client_secret.json\n  bob gmail login -m\n  bob gmail login -s compose\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gmail config; defaults to ~/.config/bob/config.yml\n  BOB_GMAIL_GOG      overrides gmail.gog and gog on PATH\n  BOB_GMAIL_BROWSER  Browser executable; selects browser sign-in (otherwise automatic)"
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
     https://myaccount.google.com/permissions.\n\nExamples:\n  bob gmail logout -y\n  bob gmail logout -a you@example.com -y\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gmail config; defaults to ~/.config/bob/config.yml\n  BOB_GMAIL_GOG      overrides gmail.gog and gog on PATH"
}

fn new_command() -> ClapCommand {
    ClapCommand::new("new")
        .about("Create a local Gmail compose note")
        .long_about(
            "Create a compose note in the configured Gmail folder. This is a local vault write: it does not log in or contact Gmail. The note opens with an empty message body; To and Subject may also be left empty while drafting.",
        )
        .disable_help_flag(true)
        .arg(bcc_arg())
        .arg(bob_dir_arg())
        .arg(cc_arg())
        .arg(human_format_arg())
        .arg(subject_arg())
        .arg(to_arg())
        .arg(help_arg())
        .after_help(
            "Examples:\n  bob gmail new\n  bob gmail new -t ada@example.com -c lee@example.com -s \"Lease renewal\"\n  bob gmail new -t 'Ada <ada@example.com>, Lee <lee@example.com>' -f json\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to BOB_DIR or ~/bob\n  BOB_CONFIG_FILE    Gmail config; defaults to ~/.config/bob/config.yml",
        )
}

fn reply_command() -> ClapCommand {
    ClapCommand::new("reply")
        .about("Add a Reply section to a saved Gmail note")
        .long_about(
            "Append one editable Reply section to a saved Gmail note. Use --all for Reply all. This is a local vault write: it does not log in or contact Gmail.",
        )
        .disable_help_flag(true)
        .arg(reply_all_arg())
        .arg(bob_dir_arg())
        .arg(human_format_arg())
        .arg(
            Arg::new("note")
                .value_name("NOTE")
                .value_parser(OsStringValueParser::new())
                .required(true)
                .help("Vault-relative path to a saved Gmail note"),
        )
        .arg(help_arg())
        .after_help(
            "Examples:\n  bob gmail reply mail/2026/261105_lease_renewal.md\n  bob gmail reply mail/2026/261105_lease_renewal.md --all -f json\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to BOB_DIR or ~/bob\n  BOB_CONFIG_FILE    Gmail config; defaults to ~/.config/bob/config.yml",
        )
}

fn account_arg() -> Arg {
    Arg::new("account")
        .long("account")
        .short('a')
        .value_name("EMAIL")
        .value_parser(OsStringValueParser::new())
        .help("Google account email; overrides gmail.account")
}

fn bcc_arg() -> Arg {
    Arg::new("bcc")
        .long("bcc")
        .short('B')
        .value_name("LIST")
        .help("Comma-separated Bcc recipients")
}

fn cc_arg() -> Arg {
    Arg::new("cc")
        .long("cc")
        .short('c')
        .value_name("LIST")
        .help("Comma-separated Cc recipients")
}

fn subject_arg() -> Arg {
    Arg::new("subject")
        .long("subject")
        .short('s')
        .value_name("TEXT")
        .help("Initial subject; may be empty while authoring")
}

fn to_arg() -> Arg {
    Arg::new("to")
        .long("to")
        .short('t')
        .value_name("LIST")
        .help("Comma-separated To recipients")
}

fn all_arg() -> Arg {
    Arg::new("all")
        .long("all")
        .short('a')
        .action(ArgAction::SetTrue)
        .help("Show every saved thread instead of the newest 20")
}

fn reply_all_arg() -> Arg {
    Arg::new("all")
        .long("all")
        .short('a')
        .action(ArgAction::SetTrue)
        .help("Include all thread participants in the Reply section")
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

fn search_format_arg() -> Arg {
    Arg::new("format")
        .long("format")
        .short('f')
        .value_name("FORMAT")
        .value_parser(["table", "json"])
        .default_value("table")
        .help("Output format: table or json")
}

fn limit_arg() -> Arg {
    Arg::new("limit")
        .long("limit")
        .short('l')
        .value_name("N")
        .value_parser(clap::value_parser!(u32).range(1..))
        .default_value("20")
        .help("Maximum threads to show (1–50; default 20)")
}

fn remote_arg() -> Arg {
    Arg::new("remote")
        .long("remote")
        .short('r')
        .action(ArgAction::SetTrue)
        .help("Fetch live new-message counts from Gmail")
}

fn save_format_arg() -> Arg {
    Arg::new("format")
        .long("format")
        .short('f')
        .value_name("FORMAT")
        .value_parser(["human", "json"])
        .default_value("human")
        .help("Output format: human or json")
}

fn body_arg() -> Arg {
    Arg::new("body")
        .long("body")
        .short('B')
        .value_name("MODE")
        .value_parser(["full", "headers"])
        .help("Body mode for this save: full or headers")
}

fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .long("dry-run")
        .short('d')
        .action(ArgAction::SetTrue)
        .help("Preview paths and structural changes without writing")
}

fn parent_arg() -> Arg {
    Arg::new("parent")
        .long("parent")
        .short('P')
        .value_name("ROUTE")
        .value_parser(OsStringValueParser::new())
        .help("Area or project parent to set on newly created notes")
}

fn quiet_arg() -> Arg {
    Arg::new("quiet")
        .long("quiet")
        .short('q')
        .action(ArgAction::SetTrue)
        .help("Suppress human success output")
}

fn rerender_arg() -> Arg {
    Arg::new("rerender")
        .long("rerender")
        .short('R')
        .action(ArgAction::SetTrue)
        .help("Rebuild managed message cards while preserving authored text")
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
    pub all: bool,
    pub bob_dir: Option<PathBuf>,
    pub format: ListFormat,
    pub remote: bool,
}

/// Typed `bob gmail list` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListArgs {
    pub all: bool,
    pub bob_dir: Option<PathBuf>,
    pub format: ListFormat,
    pub remote: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchFormat {
    Table,
    Json,
}

impl SearchFormat {
    fn from_matches(matches: &ArgMatches) -> Self {
        match matches.get_one::<String>("format").map(String::as_str) {
            Some("json") => Self::Json,
            _ => Self::Table,
        }
    }

    pub(crate) fn is_json(self) -> bool {
        matches!(self, Self::Json)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchArgs {
    pub query: Option<String>,
    pub limit: u32,
    pub format: SearchFormat,
}

impl SearchArgs {
    pub(crate) fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            query: matches
                .get_one::<OsString>("query")
                .map(|value| value.to_string_lossy().into_owned()),
            limit: *matches.get_one::<u32>("limit").unwrap_or(&20),
            format: SearchFormat::from_matches(matches),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PeekArgs {
    pub input: String,
    pub format: HumanFormat,
}

impl PeekArgs {
    pub(crate) fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            input: matches
                .get_one::<OsString>("input")
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default(),
            format: HumanFormat::from_matches(matches),
        }
    }
}

/// Typed `bob gmail link` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LinkArgs {
    pub all: bool,
    pub bob_dir: Option<PathBuf>,
    pub dry_run: bool,
    pub format: HumanFormat,
    pub notes: Vec<String>,
    pub quiet: bool,
}

impl LinkArgs {
    pub(crate) fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            all: matches.get_flag("all"),
            bob_dir: raw_bob_dir(matches),
            dry_run: matches.get_flag("dry-run"),
            format: HumanFormat::from_matches(matches),
            notes: matches
                .get_many::<OsString>("note")
                .into_iter()
                .flatten()
                .map(|value| value.to_string_lossy().into_owned())
                .collect(),
            quiet: matches.get_flag("quiet"),
        }
    }

    pub(crate) fn bob_dir(&self) -> PathBuf {
        self.bob_dir.clone().unwrap_or_else(bob_env::bob_dir)
    }
}

/// Typed `save` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SaveArgs {
    pub bob_dir: Option<PathBuf>,
    pub body: Option<crate::native::config::GmailBodyMode>,
    pub dry_run: bool,
    pub format: HumanFormat,
    pub inputs: Vec<String>,
    pub parent: Option<String>,
    pub quiet: bool,
    pub rerender: bool,
}

/// `attach` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttachArgs {
    pub bob_dir: Option<PathBuf>,
    pub dry_run: bool,
    pub format: HumanFormat,
    pub note: String,
    pub number: u32,
}

impl AttachArgs {
    pub(crate) fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            bob_dir: raw_bob_dir(matches),
            dry_run: matches.get_flag("dry-run"),
            format: HumanFormat::from_matches(matches),
            note: matches
                .get_one::<OsString>("note")
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default(),
            number: matches.get_one::<u32>("number").copied().unwrap_or(0),
        }
    }

    pub(crate) fn bob_dir(&self) -> PathBuf {
        self.bob_dir.clone().unwrap_or_else(bob_env::bob_dir)
    }
}

/// `sync` arguments (and the hidden `pull` alias).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncArgs {
    pub bob_dir: Option<PathBuf>,
    pub dry_run: bool,
    pub format: HumanFormat,
    pub limit: u32,
    pub quiet: bool,
    pub retry_timeout: u32,
}

impl SyncArgs {
    pub(crate) fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            bob_dir: raw_bob_dir(matches),
            dry_run: matches.get_flag("dry-run"),
            format: HumanFormat::from_matches(matches),
            limit: matches.get_one::<u32>("limit").copied().unwrap_or(50),
            quiet: matches.get_flag("quiet"),
            retry_timeout: matches
                .get_one::<u32>("retry-timeout")
                .copied()
                .unwrap_or(120),
        }
    }

    pub(crate) fn bob_dir(&self) -> PathBuf {
        self.bob_dir.clone().unwrap_or_else(bob_env::bob_dir)
    }
}

impl SaveArgs {
    pub(crate) fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            bob_dir: raw_bob_dir(matches),
            body: match matches.get_one::<String>("body").map(String::as_str) {
                Some("headers") => {
                    Some(crate::native::config::GmailBodyMode::Headers)
                }
                Some("full") => {
                    Some(crate::native::config::GmailBodyMode::Full)
                }
                _ => None,
            },
            dry_run: matches.get_flag("dry-run"),
            format: HumanFormat::from_matches(matches),
            inputs: matches
                .get_many::<OsString>("input")
                .into_iter()
                .flatten()
                .map(|value| value.to_string_lossy().into_owned())
                .collect(),
            parent: matches
                .get_one::<OsString>("parent")
                .map(|value| value.to_string_lossy().trim().to_string()),
            quiet: matches.get_flag("quiet"),
            rerender: matches.get_flag("rerender"),
        }
    }
}

/// Typed `new` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewArgs {
    pub bcc: Option<String>,
    pub bob_dir: Option<PathBuf>,
    pub cc: Option<String>,
    pub format: HumanFormat,
    pub subject: Option<String>,
    pub to: Option<String>,
}

impl NewArgs {
    pub(crate) fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            bcc: matches.get_one::<String>("bcc").cloned(),
            bob_dir: raw_bob_dir(matches),
            cc: matches.get_one::<String>("cc").cloned(),
            format: HumanFormat::from_matches(matches),
            subject: matches.get_one::<String>("subject").cloned(),
            to: matches.get_one::<String>("to").cloned(),
        }
    }

    pub(crate) fn bob_dir(&self) -> PathBuf {
        self.bob_dir.clone().unwrap_or_else(bob_env::bob_dir)
    }
}

/// Typed `reply` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReplyArgs {
    pub all: bool,
    pub bob_dir: Option<PathBuf>,
    pub format: HumanFormat,
    pub note: PathBuf,
}

impl ReplyArgs {
    pub(crate) fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            all: matches.get_flag("all"),
            bob_dir: raw_bob_dir(matches),
            format: HumanFormat::from_matches(matches),
            note: matches
                .get_one::<OsString>("note")
                .map(PathBuf::from)
                .unwrap_or_default(),
        }
    }

    pub(crate) fn bob_dir(&self) -> PathBuf {
        self.bob_dir.clone().unwrap_or_else(bob_env::bob_dir)
    }
}

/// Typed `draft` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DraftArgs {
    pub bob_dir: Option<PathBuf>,
    pub format: HumanFormat,
    pub note: PathBuf,
}

impl DraftArgs {
    pub(crate) fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            bob_dir: raw_bob_dir(matches),
            format: HumanFormat::from_matches(matches),
            note: matches
                .get_one::<OsString>("note")
                .map(PathBuf::from)
                .unwrap_or_default(),
        }
    }

    pub(crate) fn bob_dir(&self) -> PathBuf {
        self.bob_dir.clone().unwrap_or_else(bob_env::bob_dir)
    }
}

/// Typed `send` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SendArgs {
    pub bob_dir: Option<PathBuf>,
    pub confirm: Option<String>,
    pub dry_run: bool,
    pub format: HumanFormat,
    pub note: PathBuf,
}

impl SendArgs {
    pub(crate) fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            bob_dir: raw_bob_dir(matches),
            confirm: matches.get_one::<String>("confirm").cloned(),
            dry_run: matches.get_flag("dry-run"),
            format: HumanFormat::from_matches(matches),
            note: matches
                .get_one::<OsString>("note")
                .map(PathBuf::from)
                .unwrap_or_default(),
        }
    }

    pub(crate) fn bob_dir(&self) -> PathBuf {
        self.bob_dir.clone().unwrap_or_else(bob_env::bob_dir)
    }
}

impl TopArgs {
    pub(crate) fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            all: matches.get_flag("all"),
            bob_dir: raw_bob_dir(matches),
            format: ListFormat::from_matches(matches),
            remote: matches.get_flag("remote"),
        }
    }
}

impl ListArgs {
    pub(crate) fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            all: matches.get_flag("all"),
            bob_dir: raw_bob_dir(matches),
            format: ListFormat::from_matches(matches),
            remote: matches.get_flag("remote"),
        }
    }

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
    fn compose_command_args_match_public_flags() {
        let new = NewArgs::from_matches(&sub_matches_for(
            &[
                "bob gmail",
                "new",
                "-b",
                "~/vault",
                "-B",
                "bcc@example.com",
                "-c",
                "cc@example.com",
                "-f",
                "json",
                "-s",
                "Lease",
                "-t",
                "to@example.com",
            ],
            "new",
        ));
        assert_eq!(new.bob_dir, Some(bob_env::home_dir().join("vault")));
        assert_eq!(new.bcc.as_deref(), Some("bcc@example.com"));
        assert_eq!(new.cc.as_deref(), Some("cc@example.com"));
        assert_eq!(new.subject.as_deref(), Some("Lease"));
        assert_eq!(new.to.as_deref(), Some("to@example.com"));
        assert!(new.format.is_json());

        let reply = ReplyArgs::from_matches(&sub_matches_for(
            &[
                "bob gmail",
                "reply",
                "mail/2026/thread.md",
                "--all",
                "-f",
                "json",
            ],
            "reply",
        ));
        assert_eq!(reply.note, PathBuf::from("mail/2026/thread.md"));
        assert!(reply.all);
        assert!(reply.format.is_json());
    }

    #[test]
    fn link_args_pin_options() {
        let args = LinkArgs::from_matches(&sub_matches_for(
            &[
                "bob gmail",
                "link",
                "one.md",
                "two.md",
                "-a",
                "-b",
                "~/vault",
                "-d",
                "-f",
                "json",
                "-q",
            ],
            "link",
        ));
        assert_eq!(args.notes, vec!["one.md", "two.md"]);
        assert!(args.all);
        assert!(args.dry_run);
        assert!(args.format.is_json());
        assert!(args.quiet);
        assert_eq!(args.bob_dir(), bob_env::home_dir().join("vault"));

        let bare = LinkArgs::from_matches(&sub_matches_for(
            &["bob gmail", "link", "one.md"],
            "link",
        ));
        assert!(!bare.all);
        assert!(!bare.dry_run);
        assert!(!bare.quiet);
        assert_eq!(bare.format, HumanFormat::Human);
    }

    #[test]
    fn top_defaults_to_table() {
        let args = TopArgs::from_matches(&matches_for(&["bob gmail"]));
        assert_eq!(args.format, ListFormat::Table);
        assert!(!args.all);
    }

    #[test]
    fn sync_and_hidden_pull_alias_parse_the_same_args() {
        let flags = [
            "bob gmail",
            "SYNC",
            "-b",
            "~/vault",
            "-d",
            "-f",
            "json",
            "-l",
            "5",
            "-q",
            "-r",
            "30",
        ];
        let mut sync_flags = flags;
        sync_flags[1] = "sync";
        let mut pull_flags = flags;
        pull_flags[1] = "pull";
        let sync =
            SyncArgs::from_matches(&sub_matches_for(&sync_flags, "sync"));
        let pull =
            SyncArgs::from_matches(&sub_matches_for(&pull_flags, "sync"));
        assert_eq!(sync, pull);
        assert_eq!(sync.limit, 5);
        assert_eq!(sync.retry_timeout, 30);
        assert!(sync.dry_run);
        assert!(sync.quiet);
        assert!(sync.format.is_json());
        assert_eq!(sync.bob_dir(), bob_env::home_dir().join("vault"));
    }

    #[test]
    fn sync_defaults_limit_50_and_retry_120() {
        let args = SyncArgs::from_matches(&sub_matches_for(
            &["bob gmail", "sync"],
            "sync",
        ));
        assert_eq!(args.limit, 50);
        assert_eq!(args.retry_timeout, 120);
        assert!(!args.dry_run);
        assert!(!args.quiet);
        assert_eq!(args.format, HumanFormat::Human);
    }

    #[test]
    fn bare_and_explicit_list_share_all_and_format() {
        let top = TopArgs::from_matches(&matches_for(&[
            "bob gmail",
            "-a",
            "-b",
            "~/vault",
            "-f",
            "json",
        ]));
        assert!(top.all);
        assert!(top.format.is_json());
        assert_eq!(top.bob_dir, Some(bob_env::home_dir().join("vault")));

        let list = ListArgs::from_matches(&sub_matches_for(
            &["bob gmail", "list", "-a", "-b", "~/vault", "-f", "json"],
            "list",
        ));
        assert!(list.all);
        assert!(list.format.is_json());

        let bare = ListArgs::from_matches(&sub_matches_for(
            &["bob gmail", "list"],
            "list",
        ));
        assert!(!bare.all);
        assert_eq!(bare.format, ListFormat::Table);
    }
}

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
             through gog, then list accepted events and preview their roots. \
             Sync writes them above today's Pomodoros, idempotently.",
        )
        .after_help(top_after_help())
        .disable_help_flag(true)
        .arg(omitted_arg())
        .arg(bob_dir_arg())
        .arg(day_arg())
        .arg(list_format_arg())
        .arg(help_arg())
        .subcommand(calendars_command())
        .subcommand(doctor_command())
        .subcommand(list_command())
        .subcommand(login_command())
        .subcommand(logout_command())
        .subcommand(note_command())
        .subcommand(sync_command())
}

fn top_after_help() -> &'static str {
    "Running `bob gcal` with no command runs `bob gcal list`.\n\nExamples:\n  bob gcal                    Show today's events and what sync would do\n  bob gcal -D tomorrow        List tomorrow in Bob's timezone\n  bob gcal -a -f json         Include omitted events as JSON\n  bob gcal login              Connect Google Calendar in one guided step\n  bob gcal login -a EMAIL     Sign in as this account\n  bob gcal login -c PATH      Replace Bob's stored Desktop-app client JSON\n  bob gcal login -m           Paste the redirect address (automatic over SSH)\n  bob gcal calendars          List calendars with ids for config\n  bob gcal doctor             Diagnose gog, the client, and the vault\n  bob gcal note               List private event notes for the next 90 days\n  bob gcal note -a            List every upcoming event, noted or not\n  bob gcal note deerwood      Show one event and its private note\n  bob gcal sync               Write today's Events and MEET links\n  bob gcal sync -F            Rewrite today's Events from Google as on a first sync\n  bob gcal sync -d            Preview the exact Markdown a sync writes\n  bob gcal logout -y          Remove Bob's Calendar grant only\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gcal config; defaults to ~/.config/bob/config.yml\n  BOB_GCAL_GOG       overrides gcal.gog and gog on PATH\n  BOB_GCAL_BROWSER   Browser executable; selects browser sign-in (otherwise automatic)"
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
    "Examples:\n  bob gcal calendars\n  bob gcal calendars -f json\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gcal config; defaults to ~/.config/bob/config.yml\n  BOB_GCAL_GOG       overrides gcal.gog and gog on PATH"
}

fn list_command() -> ClapCommand {
    ClapCommand::new("list")
        .about(
            "List a local day of events and show how Bob would classify them",
        )
        .disable_help_flag(true)
        .arg(omitted_arg())
        .arg(bob_dir_arg())
        .arg(day_arg())
        .arg(list_format_arg())
        .arg(help_arg())
        .after_help(list_after_help())
}

fn list_after_help() -> &'static str {
    "`-a` also shows declined, unanswered, and out-of-scope events with their reasons. Root candidates are validated against the vault; warnings show why Bob fell back. When a task you don't organize still carries Bob lines only in the organizer's description, list prints a ⚑ hint with `bob gcal note -A -i ID -C CAL` and JSON sets `fragile_directives: true`.\n\nExamples:\n  bob gcal\n  bob gcal list -D tomorrow\n  bob gcal list -D 2026-10-30 -f json\n  bob gcal list -a\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gcal config; defaults to ~/.config/bob/config.yml\n  BOB_GCAL_GOG       overrides gcal.gog and gog on PATH\n  BOB_NOW            Local datetime override for default day selection"
}

fn note_command() -> ClapCommand {
    ClapCommand::new("note")
        .about("List, show, preview, and write private event notes")
        .long_about(
            "List, show, preview, and write private event notes: Bob-only description text \
             keyed by the Google event. Bare `bob gcal note` lists noted \
             events for the next 90 days; `-a` lists every upcoming event, \
             noted or not. A query without note text shows one event. Write \
             the same description grammar with TEXT, use `-d` to preview, \
             `-e` to edit in `$VISUAL`/`$EDITOR`/`vi`, `-A` to adopt Google \
             directive and reason lines, and `-c` to clear.",
        )
        .disable_help_flag(true)
        .arg(adopt_arg())
        .arg(all_arg())
        .arg(bob_dir_arg())
        .arg(calendar_arg())
        .arg(clear_arg())
        .arg(note_day_arg())
        .arg(note_dry_run_arg())
        .arg(edit_arg())
        .arg(event_id_arg())
        .arg(human_format_arg())
        .arg(help_arg())
        .arg(note_list_arg())
        .arg(scope_arg())
        .arg(query_arg())
        .arg(text_arg())
        .after_help(note_after_help())
}

fn note_after_help() -> &'static str {
    "Select one event by title words across the next 90 days, `-D` for one \
     day, a pasted Google Calendar URL with `eid=`, or `-i`/`-C`. Several \
     matches open a numbered picker on a TTY and exit 2 off a TTY with \
     candidates. A recurring match asks This event or All events on a TTY \
     and needs `-s` off a TTY. TEXT starts at the first directive token or \
     after `--`; argv words are joined into a line and normalized. A save on \
     the writer host immediately syncs an existing rendered day. `-e` edits \
     the selected scope in `$VISUAL`, then `$EDITOR`, then `vi` on a TTY; \
     a git-style scissors block is dropped on save. Empty text clears the \
     note. Validation errors ask `Edit again? [Y/n]`. `-A` adopts Google \
     description directive and reason lines, including bare `-N`/`+M` and \
     `-0`/`+0`; it refuses when a note already exists unless combined with \
     `-e`.\n\nExamples:\n  bob gcal note\n  bob gcal note -a -f json\n  bob gcal note -l deerwood\n  bob gcal note deerwood\n  bob gcal note deerwood @family -30 Drive there\n  bob gcal note deerwood -e\n  bob gcal note deerwood -A\n  bob gcal note -d -i event-id -- @family -30 Drive there\n  bob gcal note -c -i event-id\n  bob gcal note -D 2026-12-13 little gym\n  bob gcal note -i st49ogf0k0jiciob1ebgj1aupo\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gcal config; defaults to ~/.config/bob/config.yml\n  BOB_GCAL_GOG       overrides gcal.gog and gog on PATH\n  BOB_NOW            Local datetime override for default day selection\n  BOB_GCAL_ASSUME_TTY  Set to 1 to force interactive prompts in tests\n  VISUAL, EDITOR       Editor executable used by `-e` (`vi` if both are unset)"
}

fn sync_command() -> ClapCommand {
    ClapCommand::new("sync")
        .about("Write one day's Events rows and MEET links")
        .long_about(
            "Write one day's Events rows and MEET links: reconcile one \
             bounded, idempotent day (default today, `-D` for another day \
             whose note exists). `-d` prints the exact unified diff of the \
             plan and writes nothing. `-F` resets the day's sync journal and \
             reapplies Google over its Events rows and MEET links. Writes \
             nothing, and exits 0 with a reason and hint, while the note is \
             missing, has no `## Pomodoros`, or still has Templater tags \
             (`<% … %>`) Templater has not rendered.",
        )
        .after_help(sync_after_help())
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(day_arg())
        .arg(dry_run_arg())
        .arg(sync_force_arg())
        .arg(human_format_arg())
        .arg(help_arg())
        .arg(quiet_arg())
}

fn sync_after_help() -> &'static str {
    "Examples:\n  bob gcal sync\n  bob gcal sync -D tomorrow\n  bob gcal sync -d\n  bob gcal sync -F\n  bob gcal sync -F -d\n  bob gcal sync -q\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gcal config; defaults to ~/.config/bob/config.yml\n  BOB_GCAL_GOG       overrides gcal.gog and gog on PATH\n  BOB_NOW            Local datetime override for default day selection"
}

fn doctor_command() -> ClapCommand {
    ClapCommand::new("doctor")
        .about(
            "Check the Calendar setup: gog, client, account, token, \
             calendars, default root, today, journal, notes, and writer",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(human_format_arg())
        .arg(help_arg())
        .after_help(doctor_after_help())
}

fn doctor_after_help() -> &'static str {
    "The `notes` check reports how many private event notes fold, then warns on orphans (gone, cancelled, or declined events and series), unresolved routes, foreign-account notes, malformed `gcal.note` records, and notes past the 25-lookup cap.\n\nExamples:\n  bob gcal doctor\n  bob gcal doctor -f json\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gcal config; defaults to ~/.config/bob/config.yml\n  BOB_GCAL_GOG       overrides gcal.gog and gog on PATH"
}

fn login_command() -> ClapCommand {
    ClapCommand::new("login")
        .about("Connect Google Calendar (read-only) in one guided step")
        .long_about(
            "Connect Google Calendar (read-only) in one guided step.\n\n\
             Uses an isolated `--client bob-gcal` token bucket so removing \
             or re-consenting Bob's grant never touches other gog tokens. \
             Bob reuses a readable gog client or checks ~/Downloads before \
             guiding you through client setup. Sign-in opens a browser when \
             a desktop session is available and uses the paste-URL flow \
             automatically over SSH. Rerunning a healthy login is \
             idempotent: it reports already connected and rechecks \
             calendars. `-c FILE` replaces Bob's stored client. `-m` \
             forces paste sign-in.",
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
    "Examples:\n  bob gcal login\n  bob gcal login -a you@example.com\n  bob gcal login -c ~/Downloads/client_secret.json\n  bob gcal login -m\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gcal config; defaults to ~/.config/bob/config.yml\n  BOB_GCAL_GOG       overrides gcal.gog and gog on PATH\n  BOB_GCAL_BROWSER   Browser executable; selects browser sign-in (otherwise automatic)"
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
     https://myaccount.google.com/permissions.\n\nExamples:\n  bob gcal logout -y\n  bob gcal logout -a you@example.com -y\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    gcal config; defaults to ~/.config/bob/config.yml\n  BOB_GCAL_GOG       overrides gcal.gog and gog on PATH"
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

fn omitted_arg() -> Arg {
    Arg::new("all")
        .long("all")
        .short('a')
        .action(ArgAction::SetTrue)
        .help("Also show omitted events with the reason")
}

fn day_arg() -> Arg {
    Arg::new("day")
        .long("day")
        .short('D')
        .value_name("DAY")
        .default_value("today")
        .value_parser(OsStringValueParser::new())
        .help("Local day: today, tomorrow, yesterday, or YYYY-MM-DD")
}

fn manual_arg() -> Arg {
    Arg::new("manual")
        .long("manual")
        .short('m')
        .action(ArgAction::SetTrue)
        .help("Sign in by pasting the redirect address (automatic over SSH)")
}

fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .long("dry-run")
        .short('d')
        .action(ArgAction::SetTrue)
        .help("Preview the exact Markdown a sync would write")
}

fn sync_force_arg() -> Arg {
    Arg::new("force")
        .long("force")
        .short('F')
        .action(ArgAction::SetTrue)
        .help("Reset this day: forget your deletions and edits and rewrite its Events and MEET links from Google, as on a first sync")
}

fn quiet_arg() -> Arg {
    Arg::new("quiet")
        .long("quiet")
        .short('q')
        .action(ArgAction::SetTrue)
        .help("Print nothing on success, no-op, or waiting")
}

fn adopt_arg() -> Arg {
    Arg::new("adopt")
        .long("adopt")
        .short('A')
        .action(ArgAction::SetTrue)
        .help("Adopt Google directive and reason lines as a private note")
}

fn all_arg() -> Arg {
    Arg::new("all")
        .long("all")
        .short('a')
        .action(ArgAction::SetTrue)
        .help("List every upcoming event, noted or not")
}

fn calendar_arg() -> Arg {
    Arg::new("calendar")
        .long("calendar")
        .short('C')
        .value_name("ID")
        .value_parser(OsStringValueParser::new())
        .help("Pin the calendar for -i or a pasted URL")
}

fn clear_arg() -> Arg {
    Arg::new("clear")
        .long("clear")
        .short('c')
        .action(ArgAction::SetTrue)
        .help("Clear the private note in the selected scope")
}

fn note_day_arg() -> Arg {
    Arg::new("day")
        .long("day")
        .short('D')
        .value_name("DAY")
        .value_parser(OsStringValueParser::new())
        .help("Narrow the 90-day window to one local day, past days included")
}

fn note_dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .long("dry-run")
        .short('d')
        .action(ArgAction::SetTrue)
        .help("Validate and preview note text without saving")
}

fn edit_arg() -> Arg {
    Arg::new("edit")
        .long("edit")
        .short('e')
        .action(ArgAction::SetTrue)
        .help("Edit the note in $VISUAL, $EDITOR, or vi")
}

fn event_id_arg() -> Arg {
    Arg::new("event-id")
        .long("event-id")
        .short('i')
        .value_name("ID")
        .value_parser(OsStringValueParser::new())
        .help("Select the event by Google event id")
}

fn note_list_arg() -> Arg {
    Arg::new("list")
        .long("list")
        .short('l')
        .action(ArgAction::SetTrue)
        .help("List notes instead of showing one event")
}

fn scope_arg() -> Arg {
    Arg::new("scope")
        .long("scope")
        .short('s')
        .value_name("SCOPE")
        .value_parser(["event", "series"])
        .help("Note scope for a recurring event: event or series")
}

fn query_arg() -> Arg {
    Arg::new("query")
        .value_name("QUERY")
        .num_args(0..)
        .value_parser(OsStringValueParser::new())
        .help("Title words to select the event")
}

fn text_arg() -> Arg {
    Arg::new("text")
        .value_name("TEXT")
        .num_args(0..)
        .last(true)
        .value_parser(OsStringValueParser::new())
        .help("Note text after --")
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

/// Typed read-only `list` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListArgs {
    pub bob_dir: Option<PathBuf>,
    pub day: String,
    pub include_omitted: bool,
    pub format: ListFormat,
}

impl ListArgs {
    pub(crate) fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            bob_dir: raw_bob_dir(matches),
            day: matches
                .get_one::<OsString>("day")
                .map(|value| value.to_string_lossy().trim().to_string())
                .unwrap_or_else(|| "today".to_string()),
            include_omitted: matches.get_flag("all"),
            format: ListFormat::from_matches(matches),
        }
    }

    pub(crate) fn bob_dir(&self) -> PathBuf {
        self.bob_dir.clone().unwrap_or_else(bob_env::bob_dir)
    }

    pub(crate) fn error_format(&self) -> &'static str {
        self.format.as_str()
    }
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

/// Typed `sync` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncArgs {
    pub bob_dir: Option<PathBuf>,
    pub day: String,
    pub dry_run: bool,
    pub force: bool,
    pub format: HumanFormat,
    pub quiet: bool,
}

impl SyncArgs {
    pub(crate) fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            bob_dir: raw_bob_dir(matches),
            day: matches
                .get_one::<OsString>("day")
                .map(|value| value.to_string_lossy().trim().to_string())
                .unwrap_or_else(|| "today".to_string()),
            dry_run: matches.get_flag("dry-run"),
            force: matches.get_flag("force"),
            format: HumanFormat::from_matches(matches),
            quiet: matches.get_flag("quiet"),
        }
    }

    pub(crate) fn bob_dir(&self) -> PathBuf {
        self.bob_dir.clone().unwrap_or_else(bob_env::bob_dir)
    }

    pub(crate) fn error_format(&self) -> &'static str {
        self.format.as_str()
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

/// Typed `note` arguments: selection, editor/adoption, and plain-text writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoteArgs {
    pub adopt: bool,
    pub all: bool,
    pub bob_dir: Option<PathBuf>,
    pub calendar: Option<String>,
    pub clear: bool,
    pub day: Option<String>,
    pub dry_run: bool,
    pub edit: bool,
    pub event_id: Option<String>,
    pub format: HumanFormat,
    pub list: bool,
    pub scope: Option<String>,
    pub query: Vec<String>,
    pub text: Vec<String>,
}

impl NoteArgs {
    pub(crate) fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            adopt: matches.get_flag("adopt"),
            all: matches.get_flag("all"),
            bob_dir: raw_bob_dir(matches),
            calendar: matches
                .get_one::<OsString>("calendar")
                .map(|value| value.to_string_lossy().trim().to_string())
                .filter(|value| !value.is_empty()),
            clear: matches.get_flag("clear"),
            day: matches
                .get_one::<OsString>("day")
                .map(|value| value.to_string_lossy().trim().to_string())
                .filter(|value| !value.is_empty()),
            dry_run: matches.get_flag("dry-run"),
            edit: matches.get_flag("edit"),
            event_id: matches
                .get_one::<OsString>("event-id")
                .map(|value| value.to_string_lossy().trim().to_string())
                .filter(|value| !value.is_empty()),
            format: HumanFormat::from_matches(matches),
            list: matches.get_flag("list"),
            scope: matches
                .get_one::<String>("scope")
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            query: matches
                .get_many::<OsString>("query")
                .map(|values| {
                    values
                        .map(|value| value.to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default(),
            text: matches
                .get_many::<OsString>("text")
                .map(|values| {
                    values
                        .map(|value| value.to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    pub(crate) fn bob_dir(&self) -> PathBuf {
        self.bob_dir.clone().unwrap_or_else(bob_env::bob_dir)
    }

    pub(crate) fn error_format(&self) -> &'static str {
        self.format.as_str()
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
    fn sync_args_pin_options() {
        let args = SyncArgs::from_matches(&sub_matches_for(
            &[
                "bob gcal", "sync", "-b", "~/vault", "-D", "tomorrow", "-d",
                "-f", "json", "-q", "-F",
            ],
            "sync",
        ));
        assert_eq!(args.bob_dir(), bob_env::home_dir().join("vault"));
        assert_eq!(args.day, "tomorrow");
        assert!(args.dry_run);
        assert!(args.force);
        assert!(args.format.is_json());
        assert!(args.quiet);
        assert_eq!(args.error_format(), "json");
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

use std::{ffi::OsString, path::PathBuf, time::Duration};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde_json::json;

use crate::native::{
    env as bob_env,
    ob::{self, LockWaitError},
    plan_budget::normalize_component,
    pomodoro_homes::Hue,
};

use super::{
    journal::{self, EventPayload},
    Catalog, ThemeRecord,
};

const SCHEMA_VERSION: u32 = 1;
const LOCK_WAIT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteKind {
    Hide,
    Unhide,
    Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Human,
    Json,
}

impl OutputFormat {
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
}

pub(crate) fn run_hide(args: Vec<OsString>) -> i32 {
    run_write(WriteKind::Hide, args)
}

pub(crate) fn run_unhide(args: Vec<OsString>) -> i32 {
    run_write(WriteKind::Unhide, args)
}

pub(crate) fn run_color(args: Vec<OsString>) -> i32 {
    run_write(WriteKind::Color, args)
}

pub(crate) fn build_cli_hide() -> ClapCommand {
    build_cli(WriteKind::Hide)
}

pub(crate) fn build_cli_unhide() -> ClapCommand {
    build_cli(WriteKind::Unhide)
}

pub(crate) fn build_cli_color() -> ClapCommand {
    build_cli(WriteKind::Color)
}

fn command_name(kind: WriteKind) -> &'static str {
    match kind {
        WriteKind::Hide => "bob themes hide",
        WriteKind::Unhide => "bob themes unhide",
        WriteKind::Color => "bob themes color",
    }
}

fn run_write(kind: WriteKind, args: Vec<OsString>) -> i32 {
    let mut command = build_cli(kind);
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(command_name(kind))).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(kind, error),
    };
    let format = OutputFormat::from_matches(&matches);
    match execute(kind, &matches) {
        Ok(report) => {
            print_success(kind, &report, format);
            0
        }
        Err(error) => print_error(kind, error, format),
    }
}

fn print_clap_error(kind: WriteKind, error: clap::Error) -> i32 {
    let exit_code = error.exit_code();
    if let Err(print_error) = error.print() {
        eprintln!(
            "{}: failed to print command-line error: {print_error}",
            command_name(kind)
        );
    }
    exit_code
}

fn build_cli(kind: WriteKind) -> ClapCommand {
    let command = ClapCommand::new(command_name(kind))
        .about(about(kind))
        .long_about(long_about(kind))
        .after_help(after_help(kind))
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(via_arg())
        .arg(name_arg(kind));
    match kind {
        WriteKind::Color => command.arg(hue_arg()),
        WriteKind::Hide | WriteKind::Unhide => command,
    }
}

fn about(kind: WriteKind) -> &'static str {
    match kind {
        WriteKind::Hide => "Hide themes from Recent suggestion rows",
        WriteKind::Unhide => {
            "Show hidden themes in Recent suggestion rows again"
        }
        WriteKind::Color => "Pin or unpin a theme's hue",
    }
}

fn long_about(kind: WriteKind) -> &'static str {
    match kind {
        WriteKind::Hide => {
            "Hide one or more Pomodoro themes from Recent suggestion rows \
only. Today's ledger, typing the name, history, Stats, and colors stay \
unchanged. Using a hidden name never unhides it.\n\n\
Each hide appends one theme.hide event to .bob/themes/<host>.jsonl under \
bob_sync.lock (wait at most 3s). A theme that is already hidden is a \
no-op."
        }
        WriteKind::Unhide => {
            "Put hidden Pomodoro themes back in Recent suggestion rows.\n\n\
Each unhide appends one theme.unhide event to .bob/themes/<host>.jsonl \
under bob_sync.lock (wait at most 3s). A theme that is not hidden is a \
no-op."
        }
        WriteKind::Color => {
            "Pin a Pomodoro theme to a palette slot, or pass auto to drop \
the pin so the hue rule (exempt, area, then name hash) applies again.\n\n\
The write appends one theme.color event to .bob/themes/<host>.jsonl under \
bob_sync.lock (wait at most 3s). A pin that already matches is a no-op."
        }
    }
}

fn after_help(kind: WriteKind) -> &'static str {
    match kind {
        WriteKind::Hide => {
            "Examples:\n  \
bob themes hide BOOKS\n  \
bob themes hide FIXES BOOKS -f json\n  \
bob themes hide BOOKS --via bob-mac-capture -f json\n\n\
Environment:\n  \
BOB_CONFIG_FILE         Exact Bob config file; defaults to ~/.config/bob/config.yml\n  \
BOB_DIR                   Bob vault root when --bob-dir is omitted\n  \
BOB_NOW                   Local datetime override that pins the event timestamp\n  \
HOSTNAME                  Host label for the journal file; same slug vault-sync uses\n  \
BOB_VAULT_SYNC_LOCK_FILE  Maintenance lock; defaults to ${XDG_RUNTIME_DIR:-/tmp}/bob_sync.lock\n\n\
Exit codes:\n  \
0  wrote the hide event\n  \
1  unknown theme, unchanged, lock busy, or I/O failure\n  \
2  usage error\n\n\
See docs/pomodoro-themes.md for hide semantics and the journal contract."
        }
        WriteKind::Unhide => {
            "Examples:\n  \
bob themes unhide BOOKS\n  \
bob themes unhide FIXES -f json\n\n\
Environment:\n  \
BOB_CONFIG_FILE         Exact Bob config file; defaults to ~/.config/bob/config.yml\n  \
BOB_DIR                   Bob vault root when --bob-dir is omitted\n  \
BOB_NOW                   Local datetime override that pins the event timestamp\n  \
HOSTNAME                  Host label for the journal file; same slug vault-sync uses\n  \
BOB_VAULT_SYNC_LOCK_FILE  Maintenance lock; defaults to ${XDG_RUNTIME_DIR:-/tmp}/bob_sync.lock\n\n\
Exit codes:\n  \
0  wrote the unhide event\n  \
1  unknown theme, unchanged, lock busy, or I/O failure\n  \
2  usage error\n\n\
See docs/pomodoro-themes.md for hide semantics and the journal contract."
        }
        WriteKind::Color => {
            "Examples:\n  \
bob themes color BLOG red\n  \
bob themes color BLOG auto\n  \
bob themes color BLOG red -f json\n\n\
Slots: blue, orange, aqua, yellow, magenta, green, violet, red.\n\n\
Environment:\n  \
BOB_CONFIG_FILE         Exact Bob config file; defaults to ~/.config/bob/config.yml\n  \
BOB_DIR                   Bob vault root when --bob-dir is omitted\n  \
BOB_NOW                   Local datetime override that pins the event timestamp\n  \
HOSTNAME                  Host label for the journal file; same slug vault-sync uses\n  \
BOB_VAULT_SYNC_LOCK_FILE  Maintenance lock; defaults to ${XDG_RUNTIME_DIR:-/tmp}/bob_sync.lock\n\n\
Exit codes:\n  \
0  wrote the color event\n  \
1  unknown theme, invalid hue, unchanged, lock busy, or I/O failure\n  \
2  usage error\n\n\
See docs/pomodoro-themes.md for the hue rule and the journal contract."
        }
    }
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
        .value_parser(["human", "json"])
        .default_value("human")
        .help("Output format: human or json")
}

fn help_arg() -> Arg {
    Arg::new("help")
        .long("help")
        .short('h')
        .action(ArgAction::Help)
        .help("Show help")
}

fn via_arg() -> Arg {
    Arg::new("via")
        .long("via")
        .value_name("CLIENT")
        .hide(true)
        .help("Internal client id written on the journal event")
}

fn name_arg(kind: WriteKind) -> Arg {
    let arg = Arg::new("name")
        .value_name("NAME")
        .required(true)
        .help("Theme name as it appears in a Pomodoro tail");
    match kind {
        WriteKind::Color => arg.num_args(1),
        WriteKind::Hide | WriteKind::Unhide => arg.num_args(1..),
    }
}

fn hue_arg() -> Arg {
    Arg::new("hue")
        .value_name("HUE")
        .required(true)
        .help("Palette slot to pin, or auto to unpin")
}

struct WriteRequest {
    bob_dir: PathBuf,
    names: Vec<String>,
    hue: Option<Option<Hue>>,
    via: String,
}

fn request_from_matches(
    kind: WriteKind,
    matches: &ArgMatches,
) -> Result<WriteRequest, WriteError> {
    let names: Vec<String> = matches
        .get_many::<String>("name")
        .map(|values| values.cloned().collect())
        .unwrap_or_default();
    if names.is_empty() {
        return Err(WriteError::Usage("NAME is required".to_string()));
    }
    let hue = match kind {
        WriteKind::Color => {
            let raw = matches
                .get_one::<String>("hue")
                .map(String::as_str)
                .unwrap_or("auto");
            Some(parse_hue(raw)?)
        }
        WriteKind::Hide | WriteKind::Unhide => None,
    };
    let via = matches
        .get_one::<String>("via")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or("bob-cli")
        .to_string();
    Ok(WriteRequest {
        bob_dir: matches
            .get_one::<OsString>("bob-dir")
            .map(PathBuf::from)
            .map(|path| bob_env::expand_tilde(&path))
            .unwrap_or_else(bob_env::bob_dir),
        names,
        hue,
        via,
    })
}

fn parse_hue(raw: &str) -> Result<Option<Hue>, WriteError> {
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    Ok(Some(Hue::parse(trimmed).ok_or_else(|| {
        WriteError::InvalidHue(trimmed.to_string())
    })?))
}

enum WriteError {
    Usage(String),
    UnknownTheme(String),
    InvalidHue(String),
    Unchanged(String),
    LockBusy(String),
    Io(String),
}

impl WriteError {
    fn code(&self) -> &'static str {
        match self {
            Self::Usage(_) => "usage",
            Self::UnknownTheme(_) => "unknown_theme",
            Self::InvalidHue(_) => "invalid_hue",
            Self::Unchanged(_) => "unchanged",
            Self::LockBusy(_) => "lock_busy",
            Self::Io(_) => "io",
        }
    }

    fn message(&self) -> String {
        match self {
            Self::Usage(message)
            | Self::UnknownTheme(message)
            | Self::Unchanged(message)
            | Self::LockBusy(message)
            | Self::Io(message) => message.clone(),
            Self::InvalidHue(value) => format!(
                "invalid hue: {value} (use blue, orange, aqua, yellow, magenta, green, violet, red, or auto)"
            ),
        }
    }

    fn exit_code(&self) -> i32 {
        match self {
            Self::Usage(_) => 2,
            _ => 1,
        }
    }
}

struct WriteReport {
    names: Vec<String>,
    hue: Option<Option<Hue>>,
    via: String,
}

fn execute(
    kind: WriteKind,
    matches: &ArgMatches,
) -> Result<WriteReport, WriteError> {
    let request = request_from_matches(kind, matches)?;
    let today = bob_env::current_datetime().date();
    let catalog =
        Catalog::load(&request.bob_dir, today, false).map_err(|error| {
            WriteError::Io(format!(
                "read theme catalog in {}: {error}",
                request.bob_dir.display()
            ))
        })?;
    let records = resolve_names(&catalog, &request.names)?;
    let payloads = planned_events(kind, &catalog, &records, request.hue)?;
    if payloads.is_empty() {
        return Err(unchanged_error(kind, &records, request.hue));
    }
    write_payloads(&request.bob_dir, &payloads, &request.via)?;
    Ok(WriteReport {
        names: payloads
            .iter()
            .map(|payload| match payload {
                EventPayload::Hide { name, .. }
                | EventPayload::Unhide { name, .. }
                | EventPayload::Color { name, .. } => name.clone(),
                EventPayload::Rename { to_name, .. } => to_name.clone(),
            })
            .collect(),
        hue: request.hue,
        via: request.via,
    })
}

fn resolve_names<'a>(
    catalog: &'a Catalog,
    names: &[String],
) -> Result<Vec<&'a ThemeRecord>, WriteError> {
    let mut records = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for name in names {
        let key = normalize_component(name);
        let Some(record) = catalog.get(&key) else {
            return Err(WriteError::UnknownTheme(format!(
                "unknown theme: {name}"
            )));
        };
        if seen.insert(record.key.clone()) {
            records.push(record);
        }
    }
    Ok(records)
}

fn planned_events(
    kind: WriteKind,
    catalog: &Catalog,
    records: &[&ThemeRecord],
    hue: Option<Option<Hue>>,
) -> Result<Vec<EventPayload>, WriteError> {
    let mut payloads = Vec::new();
    for record in records {
        match kind {
            WriteKind::Hide => {
                if !catalog.is_hidden(&record.key) {
                    payloads.push(EventPayload::Hide {
                        key: record.key.clone(),
                        name: record.name.clone(),
                    });
                }
            }
            WriteKind::Unhide => {
                if catalog.is_hidden(&record.key) {
                    payloads.push(EventPayload::Unhide {
                        key: record.key.clone(),
                        name: record.name.clone(),
                    });
                }
            }
            WriteKind::Color => {
                let wanted = hue.flatten();
                if catalog.pin(&record.key) != wanted {
                    payloads.push(EventPayload::Color {
                        key: record.key.clone(),
                        name: record.name.clone(),
                        hue: wanted,
                    });
                }
            }
        }
    }
    Ok(payloads)
}

fn unchanged_error(
    kind: WriteKind,
    records: &[&ThemeRecord],
    hue: Option<Option<Hue>>,
) -> WriteError {
    let names = records
        .iter()
        .map(|record| record.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let message = match kind {
        WriteKind::Hide if records.len() == 1 => {
            format!("{names} is already hidden")
        }
        WriteKind::Hide => format!("{names} are already hidden"),
        WriteKind::Unhide if records.len() == 1 => {
            format!("{names} is not hidden")
        }
        WriteKind::Unhide => format!("{names} are not hidden"),
        WriteKind::Color => match hue.flatten() {
            Some(hue) => format!("{names} is already pinned {}", hue.name()),
            None => format!("{names} is not pinned"),
        },
    };
    WriteError::Unchanged(message)
}

fn write_payloads(
    vault: &std::path::Path,
    payloads: &[EventPayload],
    via: &str,
) -> Result<(), WriteError> {
    let _lock = ob::acquire_lock_waiting(LOCK_WAIT, || {}).map_err(|error| {
        match error {
            LockWaitError::Timeout { path } => WriteError::LockBusy(format!(
                "another Bob vault maintenance run is already active (waited on {})",
                path.display()
            )),
            other => WriteError::Io(other.to_string()),
        }
    })?;
    let host = journal::host_label();
    let ts = journal::event_timestamp();
    for payload in payloads {
        journal::append_event_at(vault, &host, payload, via, ts).map_err(
            |error| {
                WriteError::Io(format!(
                    "append theme journal in {}: {error}",
                    vault.display()
                ))
            },
        )?;
    }
    Ok(())
}

fn print_success(kind: WriteKind, report: &WriteReport, format: OutputFormat) {
    match format {
        OutputFormat::Json => {
            let payload = match kind {
                WriteKind::Hide => json!({
                    "schema_version": SCHEMA_VERSION,
                    "ok": true,
                    "hidden": report.names,
                    "via": report.via,
                }),
                WriteKind::Unhide => json!({
                    "schema_version": SCHEMA_VERSION,
                    "ok": true,
                    "unhidden": report.names,
                    "via": report.via,
                }),
                WriteKind::Color => json!({
                    "schema_version": SCHEMA_VERSION,
                    "ok": true,
                    "name": report.names.first(),
                    "hue": report.hue.flatten().map(Hue::name),
                    "pinned": report.hue.flatten().is_some(),
                    "via": report.via,
                }),
            };
            println!("{payload}");
        }
        OutputFormat::Human => {
            let names = report.names.join(", ");
            match kind {
                WriteKind::Hide => println!("✓ hid {names}"),
                WriteKind::Unhide => println!("✓ unhid {names}"),
                WriteKind::Color => match report.hue.flatten() {
                    Some(hue) => println!("✓ pinned {names} {}", hue.name()),
                    None => println!("✓ unpinned {names}"),
                },
            }
        }
    }
}

fn print_error(
    kind: WriteKind,
    error: WriteError,
    format: OutputFormat,
) -> i32 {
    let code = error.code();
    let message = error.message();
    let exit = error.exit_code();
    match format {
        OutputFormat::Json => {
            println!(
                "{}",
                json!({
                    "ok": false,
                    "code": code,
                    "message": message,
                })
            );
        }
        OutputFormat::Human => {
            eprintln!("{}: {message}", command_name(kind));
        }
    }
    exit
}

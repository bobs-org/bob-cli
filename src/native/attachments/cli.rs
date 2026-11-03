//! `bob attachments import` and `bob attachments origin`
//! (`docs/attachments.md`).
//!
//! Import copies explicit paths into the vault through the same
//! `classify_path_candidate` → `plan_attachments` path `%c` uses
//! (fresh reservations, empty indent) and never edits notes. Origin
//! resolves a vault copy back to the recorded source. Bare
//! `bob attachments` prints help: the group has no flag-only
//! read-only member.

use std::{ffi::OsString, path::PathBuf, time::Duration};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde_json::json;

use super::{
    journal,
    origin::{self, OriginError, OriginReport, OriginState},
};
use crate::native::{
    capture_clip, env as bob_env,
    ob::{self, LockWaitError},
    style::Styler,
};

const IMPORT_COMMAND: &str = "bob attachments import";
const ORIGIN_COMMAND: &str = "bob attachments origin";
const SCHEMA_VERSION: u32 = 1;
const LOCK_WAIT: Duration = Duration::from_secs(3);

pub(crate) fn run_import(args: Vec<OsString>) -> i32 {
    let mut command = build_cli_import();
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(IMPORT_COMMAND)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };
    let format = OutputFormat::from_matches(&matches);
    match execute_import(&matches) {
        Ok(report) => {
            print_import_success(&report, format);
            0
        }
        Err(error) => print_error(IMPORT_COMMAND, error, format),
    }
}

pub(crate) fn run_origin(args: Vec<OsString>) -> i32 {
    let mut command = build_cli_origin();
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(ORIGIN_COMMAND)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };
    let format = OutputFormat::from_matches(&matches);
    match execute_origin(&matches) {
        Ok(report) => {
            print_origin_success(&report, format);
            0
        }
        Err(error) => print_error(ORIGIN_COMMAND, error, format),
    }
}

fn print_clap_error(error: clap::Error) -> i32 {
    let exit_code = error.exit_code();
    if let Err(print_error) = error.print() {
        // A closed pipe renders nothing: stay quiet instead of
        // reporting the failed render, but keep clap's own exit
        // code so a usage error never reports success.
        if print_error.kind() != std::io::ErrorKind::BrokenPipe {
            eprintln!("bob attachments: failed to print command-line error: {print_error}");
        }
    }
    exit_code
}

pub(crate) fn build_cli_import() -> ClapCommand {
    ClapCommand::new(IMPORT_COMMAND)
        .about("Copy image and file paths into the vault")
        .long_about(
            "Copy one to ten explicit paths into the vault exactly as a \
`%c` clipboard line would: images land in `img/` (embedded at 400px), \
other files land in `file/`, an identical existing file is reused, and \
a name conflict gains an eight-character SHA-256 suffix. It never edits \
notes. Every copied attachment appends one `attachment.import` origin \
event to `.bob/attachments/<host>.jsonl` under `bob_sync.lock` (wait at \
most 3s); a failed append removes the files this import created.",
        )
        .after_help(
            "Examples:\n  \
bob attachments import ~/tmp/screenshots/x.png\n  \
bob attachments import -f json -- ~/tmp/a.png /tmp/b.pdf\n\n\
Environment:\n  \
BOB_DIR                   Bob vault root when --bob-dir is omitted\n  \
BOB_VAULT_SYNC_LOCK_FILE  Maintenance lock; defaults to ${XDG_RUNTIME_DIR:-/tmp}/bob_sync.lock\n  \
HOME                      Home directory for the ~ display form\n  \
NO_COLOR                  Disable colored output\n\n\
Exit codes:\n  \
0  imported every path\n  \
1  missing, not_a_file, not_absolute, too_many, lock_busy, or I/O failure\n  \
2  usage error\n\n\
See docs/attachments.md for the origin journal contract.",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(via_arg())
        .arg(path_arg())
}

pub(crate) fn build_cli_origin() -> ClapCommand {
    ClapCommand::new(ORIGIN_COMMAND)
        .about("Show where a vault attachment came from")
        .long_about(
            "Show the recorded origin of one vault-relative attachment \
path. A renamed or deduped copy still resolves through its content \
hash; an edited copy resolves through its saved path. The lookup is \
read-only: it never writes the vault.",
        )
        .after_help(
            "Examples:\n  \
bob attachments origin img/20261103_142837.png\n  \
bob attachments origin -f json -- img/20261103_142837.png\n\n\
Environment:\n  \
BOB_DIR                   Bob vault root when --bob-dir is omitted\n  \
NO_COLOR                  Disable colored output\n\n\
Exit codes:\n  \
0  resolved (including no recorded origin)\n  \
1  not_found or I/O failure\n  \
2  usage error\n\n\
See docs/attachments.md for the origin journal contract.",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(vault_path_arg())
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

fn path_arg() -> Arg {
    Arg::new("path")
        .value_name("PATH")
        .required(true)
        .num_args(1..)
        .help("Absolute image or file path to copy into the vault")
}

fn vault_path_arg() -> Arg {
    Arg::new("vault-path")
        .value_name("VAULT_PATH")
        .required(true)
        .num_args(1)
        .help("Vault-relative attachment path, e.g. img/x.png")
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

#[derive(Debug, Clone)]
struct CommandError {
    code: &'static str,
    message: String,
    exit: i32,
}

impl CommandError {
    fn failure(code: &'static str, message: String) -> Self {
        Self {
            code,
            message,
            exit: 1,
        }
    }

    fn usage(message: String) -> Self {
        Self {
            code: "usage",
            message,
            exit: 2,
        }
    }
}

fn print_error(
    command: &str,
    error: CommandError,
    format: OutputFormat,
) -> i32 {
    match format {
        OutputFormat::Json => {
            println!(
                "{}",
                json!({
                    "ok": false,
                    "code": error.code,
                    "message": error.message,
                })
            );
        }
        OutputFormat::Human => {
            eprintln!("{command}: {}", error.message);
        }
    }
    error.exit
}

#[derive(Debug, Clone)]
struct ImportedAttachment {
    source: String,
    display: String,
    saved: String,
    kind: String,
    reused: bool,
    embed: String,
    sha256: String,
}

struct ImportReport {
    attachments: Vec<ImportedAttachment>,
}

fn execute_import(matches: &ArgMatches) -> Result<ImportReport, CommandError> {
    let raws: Vec<String> = matches
        .get_many::<String>("path")
        .map(|values| values.cloned().collect())
        .unwrap_or_default();
    if raws.is_empty() {
        return Err(CommandError::usage("PATH is required".to_string()));
    }
    let vault = matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir);
    let via = matches
        .get_one::<String>("via")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or("cli")
        .to_string();
    let plan = capture_clip::plan_explicit_paths(&vault, &raws).map_err(
        |error: capture_clip::ExplicitPathError| {
            CommandError::failure(error.code, error.message)
        },
    )?;
    let _lock = ob::acquire_lock_waiting(LOCK_WAIT, || {}).map_err(|error| {
        let message = match error {
            LockWaitError::Timeout { path } => format!(
                "another Bob vault maintenance run is already active (waited on {})",
                path.display()
            ),
            other => other.to_string(),
        };
        CommandError::failure("lock_busy", message)
    })?;
    let created = plan
        .save()
        .map_err(|message| CommandError::failure("io", message))?;
    let journal_rows = plan
        .output
        .attachments
        .iter()
        .map(|attachment| {
            (
                attachment.saved.clone(),
                attachment.source.clone(),
                attachment.sha256.clone(),
            )
        })
        .collect::<Vec<_>>();
    let failures = journal::append_origin_events(&vault, &journal_rows, &via);
    if !failures.is_empty() {
        let mut message = failures
            .iter()
            .map(|failure| failure.error.clone())
            .collect::<Vec<_>>()
            .join("; ");
        if !created.is_empty() {
            let cleanup = capture_clip::cleanup_created(&created);
            append_import_cleanup_message(&mut message, &cleanup);
        }
        return Err(CommandError::failure("io", message));
    }
    Ok(ImportReport {
        attachments: plan
            .output
            .attachments
            .iter()
            .map(|attachment| ImportedAttachment {
                source: attachment.source.clone(),
                display: journal::display_for_source(&attachment.source),
                saved: attachment.saved.clone(),
                kind: match attachment.kind {
                    capture_clip::AttachmentKind::Image => "image".to_string(),
                    capture_clip::AttachmentKind::File => "file".to_string(),
                },
                reused: attachment.reused,
                embed: capture_clip::attachment_reference(attachment),
                sha256: attachment.sha256.clone(),
            })
            .collect(),
    })
}

fn append_import_cleanup_message(message: &mut String, failures: &[String]) {
    if failures.is_empty() {
        message.push_str("; removed files created by this import");
    } else {
        message.push_str("; imported-file cleanup also failed: ");
        message.push_str(&failures.join("; "));
    }
}

fn print_import_success(report: &ImportReport, format: OutputFormat) {
    match format {
        OutputFormat::Json => {
            let attachments = report
                .attachments
                .iter()
                .map(|attachment| {
                    json!({
                        "source": attachment.source,
                        "display": attachment.display,
                        "saved": attachment.saved,
                        "kind": attachment.kind,
                        "reused": attachment.reused,
                        "embed": attachment.embed,
                        "sha256": attachment.sha256,
                    })
                })
                .collect::<Vec<_>>();
            println!(
                "{}",
                json!({
                    "schema_version": SCHEMA_VERSION,
                    "attachments": attachments,
                })
            );
        }
        OutputFormat::Human => {
            let styler = Styler::detect();
            for attachment in &report.attachments {
                let verb = if attachment.reused {
                    styler.yellow(&format!("{:<10}", "reused"))
                } else {
                    styler.green(&format!("{:<10}", "imported"))
                };
                println!("{verb}{} → {}", attachment.display, attachment.saved);
            }
        }
    }
}

struct OriginCliReport {
    path: String,
    origin: Option<OriginReport>,
    warnings: Vec<String>,
}

fn execute_origin(
    matches: &ArgMatches,
) -> Result<OriginCliReport, CommandError> {
    let path = matches
        .get_one::<String>("vault-path")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CommandError::usage("VAULT_PATH is required".to_string())
        })?;
    let vault = matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir);
    let host = journal::host_label();
    match origin::resolve_origin(&vault, &path, &host) {
        Ok((report, warnings)) => Ok(OriginCliReport {
            path,
            origin: report,
            warnings,
        }),
        Err(error @ OriginError::NotFound(_)) => {
            Err(CommandError::failure(error.code(), error.message()))
        }
        Err(error @ OriginError::Io(_)) => {
            Err(CommandError::failure(error.code(), error.message()))
        }
    }
}

fn print_origin_success(report: &OriginCliReport, format: OutputFormat) {
    match format {
        OutputFormat::Json => {
            let origin = report.origin.as_ref().map(|origin| {
                json!({
                    "source": origin.source,
                    "display": origin.display,
                    "host": origin.host,
                    "ts": origin.ts.to_rfc3339_opts(
                        chrono::SecondsFormat::Secs, false),
                    "via": origin.via,
                    "state": origin.state.as_str(),
                })
            });
            println!(
                "{}",
                json!({
                    "schema_version": SCHEMA_VERSION,
                    "path": report.path,
                    "origin": origin,
                    "warnings": report.warnings,
                })
            );
        }
        OutputFormat::Human => match &report.origin {
            Some(origin) => {
                println!(
                    "{} ← {}  {} · {} · {} · {}",
                    report.path,
                    origin.display,
                    origin_state_label(origin.state),
                    origin.host,
                    origin.ts.format("%Y-%m-%d %H:%M"),
                    origin.via,
                );
            }
            None => {
                println!("{} has no recorded origin", report.path);
            }
        },
    }
}

fn origin_state_label(state: OriginState) -> &'static str {
    state.as_str()
}

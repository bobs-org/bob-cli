use std::{ffi::OsString, path::PathBuf, time::Duration};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde_json::json;

use crate::native::{
    env as bob_env,
    ob::{self, LockWaitError},
    task_relations::label::normalize_relation_label,
};

use super::{
    journal::{self, EventPayload},
    relation_label_key, Catalog,
};

const SCHEMA_VERSION: u32 = 1;
const LOCK_WAIT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteKind {
    Hide,
    Unhide,
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

pub(crate) fn build_cli_hide() -> ClapCommand {
    build_cli(WriteKind::Hide)
}

pub(crate) fn build_cli_unhide() -> ClapCommand {
    build_cli(WriteKind::Unhide)
}

fn command_name(kind: WriteKind) -> &'static str {
    match kind {
        WriteKind::Hide => "bob relations hide",
        WriteKind::Unhide => "bob relations unhide",
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
        .arg(allow_unused_arg(kind))
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(via_arg())
        .arg(label_arg());
    command
}

fn about(kind: WriteKind) -> &'static str {
    match kind {
        WriteKind::Hide => "Hide relation labels from suggestions",
        WriteKind::Unhide => "Show hidden relation labels in suggestions again",
    }
}

fn long_about(kind: WriteKind) -> &'static str {
    match kind {
        WriteKind::Hide => {
            "Hide one or more relation labels from suggestions only: the \
card rows, the Obsidian stage, `relations list` defaults, and shell \
completion. LINKS text, LINKED FROM mirrors, chips, counts, and typed \
use are unchanged, and using a hidden label never unhides it.\n\n\
Each hide appends one relation_label.hide event to \
.bob/relations/<host>.jsonl under bob_sync.lock (wait at most 3s). A \
label that is already hidden is a no-op."
        }
        WriteKind::Unhide => {
            "Put hidden relation labels back in suggestion rows.\n\n\
Each unhide appends one relation_label.unhide event to \
.bob/relations/<host>.jsonl under bob_sync.lock (wait at most 3s). A \
label that is not hidden is a no-op."
        }
    }
}

fn after_help(kind: WriteKind) -> &'static str {
    match kind {
        WriteKind::Hide => {
            "Examples:\n  \
bob relations hide \"inspired by\"\n  \
bob relations hide \"inspired by\" implements -f json\n  \
bob relations hide \"brand new\" --allow-unused\n\n\
Environment:\n  \
BOB_CONFIG_FILE         Exact Bob config file; defaults to ~/.config/bob/config.yml\n  \
BOB_DIR                   Bob vault root when --bob-dir is omitted\n  \
HOSTNAME                  Host label for the journal file; same slug vault-sync uses\n  \
BOB_VAULT_SYNC_LOCK_FILE  Maintenance lock; defaults to ${XDG_RUNTIME_DIR:-/tmp}/bob_sync.lock\n\n\
Exit codes:\n  \
0  wrote the hide event\n  \
1  unknown label, invalid label, unchanged, lock busy, or I/O failure\n  \
2  usage error\n\n\
See docs/task-relations.md for hide semantics and the journal contract."
        }
        WriteKind::Unhide => {
            "Examples:\n  \
bob relations unhide \"inspired by\"\n  \
bob relations unhide \"inspired by\" -f json\n\n\
Environment:\n  \
BOB_CONFIG_FILE         Exact Bob config file; defaults to ~/.config/bob/config.yml\n  \
BOB_DIR                   Bob vault root when --bob-dir is omitted\n  \
HOSTNAME                  Host label for the journal file; same slug vault-sync uses\n  \
BOB_VAULT_SYNC_LOCK_FILE  Maintenance lock; defaults to ${XDG_RUNTIME_DIR:-/tmp}/bob_sync.lock\n\n\
Exit codes:\n  \
0  wrote the unhide event\n  \
1  unknown label, invalid label, unchanged, lock busy, or I/O failure\n  \
2  usage error\n\n\
See docs/task-relations.md for hide semantics and the journal contract."
        }
    }
}

fn allow_unused_arg(kind: WriteKind) -> Arg {
    let arg = Arg::new("allow-unused")
        .long("allow-unused")
        .action(ArgAction::SetTrue)
        .help("Hide a label the catalog has not seen yet");
    match kind {
        WriteKind::Hide => arg,
        WriteKind::Unhide => arg.hide(true),
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

fn label_arg() -> Arg {
    Arg::new("label")
        .value_name("LABEL")
        .required(true)
        .num_args(1..)
        .help("Relation label as it appears on a LINKS entry")
}

struct WriteRequest {
    bob_dir: PathBuf,
    labels: Vec<String>,
    allow_unused: bool,
    via: String,
}

fn request_from_matches(
    matches: &ArgMatches,
) -> Result<WriteRequest, WriteError> {
    let labels: Vec<String> = matches
        .get_many::<String>("label")
        .map(|values| values.cloned().collect())
        .unwrap_or_default();
    if labels.is_empty() {
        return Err(WriteError::Usage("LABEL is required".to_string()));
    }
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
        labels,
        allow_unused: matches.get_flag("allow-unused"),
        via,
    })
}

enum WriteError {
    Usage(String),
    UnknownLabel(String),
    InvalidLabel(String),
    Unchanged(String),
    LockBusy(String),
    Io(String),
}

impl WriteError {
    fn code(&self) -> &'static str {
        match self {
            Self::Usage(_) => "usage",
            Self::UnknownLabel(_) => "unknown_label",
            Self::InvalidLabel(_) => "invalid_label",
            Self::Unchanged(_) => "unchanged",
            Self::LockBusy(_) => "lock_busy",
            Self::Io(_) => "io",
        }
    }

    fn message(&self) -> String {
        match self {
            Self::Usage(message)
            | Self::UnknownLabel(message)
            | Self::InvalidLabel(message)
            | Self::Unchanged(message)
            | Self::LockBusy(message)
            | Self::Io(message) => message.clone(),
        }
    }

    fn exit_code(&self) -> i32 {
        match self {
            Self::Usage(_) => 2,
            _ => 1,
        }
    }
}

struct PlannedLabel {
    key: String,
    name: String,
    count: u64,
}

struct WriteReport {
    labels: Vec<PlannedLabel>,
    via: String,
}

fn execute(
    kind: WriteKind,
    matches: &ArgMatches,
) -> Result<WriteReport, WriteError> {
    let request = request_from_matches(matches)?;
    let catalog = Catalog::load(&request.bob_dir).map_err(|error| {
        WriteError::Io(format!(
            "read relation catalog in {}: {error}",
            request.bob_dir.display()
        ))
    })?;
    let planned = resolve_labels(kind, &catalog, &request)?;
    if planned.is_empty() {
        return Err(unchanged_error(kind, &request.labels, &catalog));
    }
    write_payloads(kind, &request.bob_dir, &planned, &request.via)?;
    Ok(WriteReport {
        labels: planned,
        via: request.via,
    })
}

fn resolve_labels(
    kind: WriteKind,
    catalog: &Catalog,
    request: &WriteRequest,
) -> Result<Vec<PlannedLabel>, WriteError> {
    let mut planned = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for raw in &request.labels {
        let normalized = normalize_relation_label(raw).map_err(|_| {
            WriteError::InvalidLabel(format!("invalid label: {raw}"))
        })?;
        let key = relation_label_key(&normalized);
        if !seen.insert(key.clone()) {
            continue;
        }
        match catalog.get(&key) {
            Some(record) => {
                let hidden = catalog.is_hidden(&key);
                let changed = match kind {
                    WriteKind::Hide => !hidden,
                    WriteKind::Unhide => hidden,
                };
                if changed {
                    planned.push(PlannedLabel {
                        key: record.key.clone(),
                        name: record.name.clone(),
                        count: record.count,
                    });
                }
            }
            None => {
                if kind == WriteKind::Hide && request.allow_unused {
                    planned.push(PlannedLabel {
                        key: key.clone(),
                        name: normalized.clone(),
                        count: 0,
                    });
                } else {
                    return Err(WriteError::UnknownLabel(format!(
                        "unknown label: {raw}"
                    )));
                }
            }
        }
    }
    Ok(planned)
}

fn unchanged_error(
    kind: WriteKind,
    raw: &[String],
    catalog: &Catalog,
) -> WriteError {
    let names = raw
        .iter()
        .map(|label| {
            let key = relation_label_key(label);
            catalog
                .get(&key)
                .map(|record| record.name.clone())
                .unwrap_or_else(|| label.clone())
        })
        .collect::<Vec<_>>()
        .join(", ");
    let message = match kind {
        WriteKind::Hide if raw.len() == 1 => {
            format!("{names} is already hidden")
        }
        WriteKind::Hide => format!("{names} are already hidden"),
        WriteKind::Unhide if raw.len() == 1 => {
            format!("{names} is not hidden")
        }
        WriteKind::Unhide => format!("{names} are not hidden"),
    };
    WriteError::Unchanged(message)
}

fn write_payloads(
    kind: WriteKind,
    vault: &std::path::Path,
    planned: &[PlannedLabel],
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
    for label in planned {
        let payload = match kind {
            WriteKind::Hide => EventPayload::Hide {
                key: label.key.clone(),
                name: label.name.clone(),
            },
            WriteKind::Unhide => EventPayload::Unhide {
                key: label.key.clone(),
                name: label.name.clone(),
            },
        };
        journal::append_event_at(vault, &host, &payload, via, ts).map_err(
            |error| {
                WriteError::Io(format!(
                    "append relation journal in {}: {error}",
                    vault.display()
                ))
            },
        )?;
    }
    Ok(())
}

pub(crate) fn shell_word(name: &str) -> String {
    if name.contains([' ', '\t', '"', '\'']) {
        format!("\"{}\"", name.replace('"', "\\\""))
    } else {
        name.to_string()
    }
}

/// One `relation_label.hide` journal append the capture commit could not
/// write. Failures never fail the capture: the caller reports each one
/// with its retry command.
pub(crate) struct FailedHide {
    pub(crate) name: String,
    pub(crate) error: String,
}

/// Append one `relation_label.hide` event per label after a successful
/// capture commit, under the same lock discipline as `bob relations
/// hide`. When the lock itself is busy, every label fails with the lock
/// error and no event is written.
pub(crate) fn append_hide_events(
    vault: &std::path::Path,
    labels: &[(String, String)],
    via: &str,
) -> Vec<FailedHide> {
    if labels.is_empty() {
        return Vec::new();
    }
    let failed = |error: String| {
        labels
            .iter()
            .map(|(_, name)| FailedHide {
                name: name.clone(),
                error: error.clone(),
            })
            .collect()
    };
    let _lock = match ob::acquire_lock_waiting(LOCK_WAIT, || {}) {
        Ok(lock) => lock,
        Err(error) => {
            let message = match error {
                LockWaitError::Timeout { path } => format!(
                    "another Bob vault maintenance run is already active \
                     (waited on {})",
                    path.display()
                ),
                other => other.to_string(),
            };
            return failed(message);
        }
    };
    let host = journal::host_label();
    let ts = journal::event_timestamp();
    let mut failures = Vec::new();
    for (key, name) in labels {
        let payload = EventPayload::Hide {
            key: key.clone(),
            name: name.clone(),
        };
        if let Err(error) =
            journal::append_event_at(vault, &host, &payload, via, ts)
        {
            failures.push(FailedHide {
                name: name.clone(),
                error: format!(
                    "append relation journal in {}: {error}",
                    vault.display()
                ),
            });
        }
    }
    failures
}

fn print_success(kind: WriteKind, report: &WriteReport, format: OutputFormat) {
    match format {
        OutputFormat::Json => {
            let names = report
                .labels
                .iter()
                .map(|label| label.name.clone())
                .collect::<Vec<_>>();
            let payload = match kind {
                WriteKind::Hide => json!({
                    "schema_version": SCHEMA_VERSION,
                    "ok": true,
                    "hidden": names,
                    "via": report.via,
                }),
                WriteKind::Unhide => json!({
                    "schema_version": SCHEMA_VERSION,
                    "ok": true,
                    "unhidden": names,
                    "via": report.via,
                }),
            };
            println!("{payload}");
        }
        OutputFormat::Human => {
            for label in &report.labels {
                match kind {
                    WriteKind::Hide => {
                        let links = if label.count == 1 {
                            "1 link keeps".to_string()
                        } else {
                            format!("{} links keep", label.count)
                        };
                        println!(
                            "🪢 Hid \u{201c}{}\u{201d} from relation suggestions · {links} their label · undo: bob relations unhide {}",
                            label.name,
                            shell_word(&label.name)
                        );
                    }
                    WriteKind::Unhide => {
                        println!(
                            "🪢 Unhid \u{201c}{}\u{201d} · it is suggested again · undo: bob relations hide {}",
                            label.name,
                            shell_word(&label.name)
                        );
                    }
                }
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

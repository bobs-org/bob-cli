//! `bob query` CLI: request model, argument parsing, and validation.

use super::super::env as bob_env;
use super::*;
use clap::{
    builder::{NonEmptyStringValueParser, OsStringValueParser},
    error::ErrorKind,
    Arg, ArgAction, ArgGroup, ArgMatches, Command as ClapCommand,
};
use std::{
    ffi::{OsStr, OsString},
    fs,
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Request {
    pub(super) query: QueryInput,
    pub(super) format: OutputFormat,
    pub(super) engine: Engine,
    pub(super) vault: VaultConfig,
    pub(super) strict_paths: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum QueryInput {
    Source(String),
    Dql(DqlInput),
    Tasks(TasksInput),
    TasksNote(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum DqlInput {
    Inline(String),
    File(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TasksInput {
    Inline(String),
    File(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OutputFormat {
    Json,
    Markdown,
    Paths,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Engine {
    Native,
    Obsidian,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VaultConfig {
    pub(super) bob_dir: PathBuf,
    pub(super) origin: Option<PathBuf>,
    pub(super) obsidian_vault: Option<String>,
}

pub(crate) fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Run Dataview or Obsidian Tasks queries against the Bob vault")
        .long_about(
            "Run Dataview source expressions, Dataview DQL, or Obsidian Tasks \
queries against the Bob vault.\n\n\
The default native engine is headless and local. Dataview DQL supports paths, \
JSON, and markdown output. Native Tasks queries support Tasks v8 filters, \
JavaScript by-function instructions, sorting, grouping, layout, and whole-note \
block execution. The explicit Obsidian engine runs \
Dataview queries against the live plugin when exact installed-plugin behavior \
is needed.",
        )
        .after_help(
            "Examples:\n  bob query --source '#project and -\"archive\"'\n  bob query --query 'LIST FROM #waiting'\n  bob query --format json --query-file ~/queries/projects.dql\n  bob query --tasks 'status.type is TODO' --origin dash.md\n  bob query --format json --tasks-file ~/queries/tasks.txt\n  bob query --tasks-note dash.md --format markdown",
        )
        .disable_help_flag(true)
        .arg_required_else_help(true)
        .group(
            ArgGroup::new("query-input")
                .required(true)
                .multiple(false)
                .args([
                    "query",
                    "query-file",
                    "source",
                    "tasks",
                    "tasks-file",
                    "tasks-note",
                ]),
        )
        .arg(bob_dir_arg())
        .arg(engine_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(origin_arg())
        .arg(query_arg())
        .arg(query_file_arg())
        .arg(source_arg())
        .arg(strict_paths_arg())
        .arg(tasks_arg())
        .arg(tasks_file_arg())
        .arg(tasks_note_arg())
        .arg(vault_arg())
}

pub(super) fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .long("bob-dir")
        .short('b')
        .value_name("PATH")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

pub(super) fn engine_arg() -> Arg {
    Arg::new("engine")
        .long("engine")
        .short('e')
        .value_name("ENGINE")
        .default_value("native")
        .value_parser(["native", "obsidian"])
        .help("Query engine: native for Dataview and Tasks, obsidian for live Dataview")
}

pub(super) fn format_arg() -> Arg {
    Arg::new("format")
        .long("format")
        .short('f')
        .value_name("FORMAT")
        .default_value("paths")
        .value_parser(["json", "markdown", "paths"])
        .help("Output format: paths, structured json, or rendered markdown")
}

pub(super) fn help_arg() -> Arg {
    Arg::new("help")
        .long("help")
        .short('h')
        .action(ArgAction::Help)
        .help("Print help")
}

pub(super) fn origin_arg() -> Arg {
    Arg::new("origin")
        .long("origin")
        .short('o')
        .value_name("VAULT_RELATIVE_PATH")
        .value_parser(OsStringValueParser::new())
        .help("Origin note for Dataview this or Tasks query context")
}

pub(super) fn query_arg() -> Arg {
    Arg::new("query")
        .long("query")
        .short('q')
        .value_name("DQL")
        .value_parser(NonEmptyStringValueParser::new())
        .help("Full Dataview DQL query")
}

pub(super) fn query_file_arg() -> Arg {
    Arg::new("query-file")
        .long("query-file")
        .short('Q')
        .value_name("PATH")
        .value_parser(OsStringValueParser::new())
        .help("Read a Dataview DQL query from a file; use - for stdin")
}

pub(super) fn source_arg() -> Arg {
    Arg::new("source")
        .long("source")
        .short('s')
        .value_name("SOURCE")
        .value_parser(NonEmptyStringValueParser::new())
        .help("Dataview source expression for page path lookup")
}

pub(super) fn strict_paths_arg() -> Arg {
    Arg::new("strict-paths")
        .long("strict-paths")
        .short('S')
        .action(ArgAction::SetTrue)
        .help("Fail when paths output cannot derive clean note paths")
}

pub(super) fn tasks_arg() -> Arg {
    Arg::new("tasks")
        .long("tasks")
        .short('t')
        .value_name("QUERY")
        .help("Inline Obsidian Tasks query")
}

pub(super) fn tasks_file_arg() -> Arg {
    Arg::new("tasks-file")
        .long("tasks-file")
        .short('T')
        .value_name("PATH")
        .value_parser(OsStringValueParser::new())
        .help("Read an Obsidian Tasks query from a file; use - for stdin")
}

pub(super) fn tasks_note_arg() -> Arg {
    Arg::new("tasks-note")
        .long("tasks-note")
        .short('n')
        .value_name("VAULT_RELATIVE_PATH")
        .value_parser(OsStringValueParser::new())
        .help("Run every Tasks code block in a vault note")
}

pub(super) fn vault_arg() -> Arg {
    Arg::new("vault")
        .long("vault")
        .short('v')
        .value_name("NAME_OR_ID")
        .value_parser(NonEmptyStringValueParser::new())
        .help(
            "Obsidian engine vault name or ID; defaults to BOB_DATAVIEW_VAULT",
        )
}

impl Request {
    pub(super) fn from_matches(
        matches: &ArgMatches,
        command: &mut ClapCommand,
    ) -> Result<Self, clap::Error> {
        let mut query = QueryInput::from_matches(matches);
        let format = OutputFormat::from_matches(matches);
        let engine = Engine::from_matches(matches);
        let strict_paths = matches.get_flag("strict-paths");

        if query.is_source() && format == OutputFormat::Markdown {
            return Err(command.error(
                ErrorKind::ArgumentConflict,
                "--format markdown requires a DQL query",
            ));
        }

        if query.is_tasks() && engine == Engine::Obsidian {
            return Err(command.error(
                ErrorKind::ArgumentConflict,
                "--engine obsidian does not support Tasks queries; use \
                 --engine native (the live Tasks oracle is available through \
                 the parity harness)",
            ));
        }

        if let QueryInput::TasksNote(path) = &mut query {
            validate_vault_relative_argument(path, "--tasks-note", command)?;
            *path = normalize_vault_relative_path(path);
            if matches.get_one::<OsString>("origin").is_some() {
                return Err(command.error(
                    ErrorKind::ArgumentConflict,
                    "--origin cannot be used with --tasks-note; the note is its \
                     own query origin",
                ));
            }
        }

        if strict_paths && format != OutputFormat::Paths {
            return Err(command.error(
                ErrorKind::ArgumentConflict,
                "--strict-paths can only be used with --format paths",
            ));
        }

        if engine == Engine::Native
            && matches.get_one::<String>("vault").is_some()
        {
            return Err(command.error(
                ErrorKind::ArgumentConflict,
                "--vault can only be used with --engine obsidian",
            ));
        }

        Ok(Self {
            query,
            format,
            engine,
            vault: VaultConfig::from_matches(
                matches,
                command,
                engine == Engine::Native,
                engine == Engine::Obsidian,
            )?,
            strict_paths,
        })
    }
}
impl QueryInput {
    pub(super) fn from_matches(matches: &ArgMatches) -> Self {
        if let Some(source) = matches.get_one::<String>("source") {
            return Self::Source(source.clone());
        }

        if let Some(query) = matches.get_one::<String>("query") {
            return Self::Dql(DqlInput::Inline(query.clone()));
        }

        if let Some(query) = matches.get_one::<String>("tasks") {
            return Self::Tasks(TasksInput::Inline(query.clone()));
        }

        if let Some(query_file) = matches.get_one::<OsString>("tasks-file") {
            return Self::Tasks(TasksInput::File(query_file.into()));
        }

        if let Some(note) = matches.get_one::<OsString>("tasks-note") {
            return Self::TasksNote(note.into());
        }

        let query_file = matches
            .get_one::<OsString>("query-file")
            .expect("clap query-input group requires query-file")
            .into();
        Self::Dql(DqlInput::File(query_file))
    }

    pub(super) fn is_source(&self) -> bool {
        matches!(self, Self::Source(_))
    }

    pub(super) fn is_tasks(&self) -> bool {
        matches!(self, Self::Tasks(_) | Self::TasksNote(_))
    }
}

impl DqlInput {
    pub(super) fn read_query(&self) -> Result<String, DataviewError> {
        match self {
            Self::Inline(query) => Ok(query.clone()),
            Self::File(path) if path.as_os_str() == OsStr::new("-") => {
                let mut query = String::new();
                io::stdin().read_to_string(&mut query).map_err(|error| {
                    DataviewError::QueryRead { path: None, error }
                })?;
                Ok(query)
            }
            Self::File(path) => fs::read_to_string(path).map_err(|error| {
                DataviewError::QueryRead {
                    path: Some(path.clone()),
                    error,
                }
            }),
        }
    }
}

impl TasksInput {
    pub(super) fn read_query(&self) -> Result<String, DataviewError> {
        match self {
            Self::Inline(query) => Ok(query.clone()),
            Self::File(path) if path.as_os_str() == OsStr::new("-") => {
                let mut query = String::new();
                io::stdin().read_to_string(&mut query).map_err(|error| {
                    DataviewError::QueryRead { path: None, error }
                })?;
                Ok(query)
            }
            Self::File(path) => fs::read_to_string(path).map_err(|error| {
                DataviewError::QueryRead {
                    path: Some(path.clone()),
                    error,
                }
            }),
        }
    }
}

impl OutputFormat {
    pub(super) fn from_matches(matches: &ArgMatches) -> Self {
        match matches
            .get_one::<String>("format")
            .expect("clap provides a default format")
            .as_str()
        {
            "json" => Self::Json,
            "markdown" => Self::Markdown,
            "paths" => Self::Paths,
            value => unreachable!("unexpected format value from clap: {value}"),
        }
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Markdown => "markdown",
            Self::Paths => "paths",
        }
    }
}

impl Engine {
    pub(super) fn from_matches(matches: &ArgMatches) -> Self {
        match matches
            .get_one::<String>("engine")
            .expect("clap provides a default engine")
            .as_str()
        {
            "native" => Self::Native,
            "obsidian" => Self::Obsidian,
            value => unreachable!("unexpected engine value from clap: {value}"),
        }
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Obsidian => "obsidian",
        }
    }
}

impl VaultConfig {
    pub(super) fn from_matches(
        matches: &ArgMatches,
        command: &mut ClapCommand,
        validate_default_bob_dir: bool,
        use_obsidian_vault: bool,
    ) -> Result<Self, clap::Error> {
        let bob_dir_arg = matches.get_one::<OsString>("bob-dir");
        let bob_dir = bob_dir_arg
            .map(PathBuf::from)
            .map(|path| bob_env::expand_tilde(&path))
            .unwrap_or_else(bob_env::bob_dir);
        if bob_dir_arg.is_some() || validate_default_bob_dir {
            validate_bob_dir(&bob_dir, command)?;
        }

        let origin = matches
            .get_one::<OsString>("origin")
            .map(PathBuf::from)
            .map(|path| {
                validate_origin_path(&path, command)?;
                Ok::<PathBuf, clap::Error>(normalize_vault_relative_path(&path))
            })
            .transpose()?;
        let obsidian_vault = use_obsidian_vault
            .then(|| {
                matches
                    .get_one::<String>("vault")
                    .cloned()
                    .or_else(default_vault_from_env)
            })
            .flatten();

        Ok(Self {
            bob_dir,
            origin,
            obsidian_vault,
        })
    }
}

pub(super) fn validate_bob_dir(
    bob_dir: &Path,
    command: &mut ClapCommand,
) -> Result<(), clap::Error> {
    if bob_dir.is_dir() {
        return Ok(());
    }

    Err(command.error(
        ErrorKind::ValueValidation,
        format!(
            "--bob-dir must name an existing Bob vault directory: {}",
            bob_dir.display()
        ),
    ))
}

pub(super) fn validate_origin_path(
    origin: &Path,
    command: &mut ClapCommand,
) -> Result<(), clap::Error> {
    validate_vault_relative_path(origin).map_err(|reason| {
        command.error(
            ErrorKind::ValueValidation,
            format!("invalid --origin {}: {reason}", origin.display()),
        )
    })
}

pub(super) fn validate_vault_relative_argument(
    path: &Path,
    argument: &str,
    command: &mut ClapCommand,
) -> Result<(), clap::Error> {
    validate_vault_relative_path(path).map_err(|reason| {
        command.error(
            ErrorKind::ValueValidation,
            format!("invalid {argument} {}: {reason}", path.display()),
        )
    })
}

pub(super) fn validate_vault_relative_path(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty() {
        return Err("path must not be empty".to_string());
    }
    if path.is_absolute() {
        return Err("absolute paths are not allowed".to_string());
    }
    if path.to_string_lossy().contains('\0') {
        return Err("NUL bytes are not allowed".to_string());
    }

    for component in path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => {
                return Err(".. traversal is not allowed".to_string());
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err("absolute paths are not allowed".to_string());
            }
        }
    }

    Ok(())
}

pub(super) fn normalize_vault_relative_path(path: &Path) -> PathBuf {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value),
            Component::CurDir => None,
            Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => None,
        })
        .collect()
}

pub(super) fn default_vault_from_env() -> Option<String> {
    bob_env::var(ENV_VAULT)
        .ok()
        .filter(|value| !value.is_empty())
}

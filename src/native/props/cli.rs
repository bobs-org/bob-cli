//! Guarded `bob props show` and `bob props update` commands.
//!
//! `show` renders the ordered property report for one vault note and is
//! read-only. `update` applies a guarded JSON envelope of `prop.*`
//! operations against a semantic `expect` fingerprint under the vault lock,
//! replanning against bytes re-read under that lock. Neither command runs
//! vault-sync, writes Git commits, or creates cache entries; only a changed
//! non-dry-run apply writes, and it installs atomically.

use std::{
    ffi::{OsStr, OsString},
    fs, io,
    io::Read,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use clap::{
    builder::{OsStringValueParser, PossibleValue, PossibleValuesParser},
    Arg, ArgAction, ArgMatches, Command as ClapCommand,
};
use serde_json::{json, Value};

use super::{
    document::PropsDocument,
    inspect,
    plan::plan,
    protocol::{parse_envelope, Op},
    roles::resolve_note_context,
    show::ShowReport,
    InspectContext, PropsError,
};
use crate::native::{
    env as bob_env, ob, records::context::RecordContext, style::Styler,
};

pub(crate) const COMMAND_SHOW: &str = "bob props show";
pub(crate) const COMMAND_UPDATE: &str = "bob props update";
const LOCK_WAIT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Human,
    Json,
}

/// Build the `bob props show` completion/runtime command.
pub(crate) fn show_command() -> ClapCommand {
    ClapCommand::new(COMMAND_SHOW)
        .about("Show ordered frontmatter properties for one note")
        .long_about(
            "Show the ordered frontmatter properties for one vault note: \
             source order, line numbers, typed values, kinds, styles, and \
             the owning card or writer for each key, plus the semantic \
             fingerprint. Area, project, and bank notes also report the \
             settings contract (stage, choices, declared and effective \
             values, and ^prj facts). NOTE is a vault-relative Markdown \
             path (the `.md` suffix is optional) or an absolute Markdown \
             path inside the vault. Damaged YAML still reports its \
             recovery state. The command is read-only. See docs/props.md \
             for the report contract and kind/style rules.",
        )
        .after_help(
            "Examples:\n  \
             bob props show notes/groceries.md\n  \
             bob props show eat/ninja -f json\n  \
             bob props show 2026/20261122 -f json\n\n\
             Environment:\n  \
             BOB_DIR          Bob vault root when --bob-dir is omitted\n  \
             NO_COLOR         Disable colored output\n\n\
             Exit codes:\n  \
             0  report\n  \
             1  vault I/O failure\n  \
             2  bad note path, missing note, or non-Markdown target\n\n\
             See docs/props.md for the report contract and kind/style rules.",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(note_arg())
}

/// Build the `bob props update` completion/runtime command.
pub(crate) fn update_command() -> ClapCommand {
    ClapCommand::new(COMMAND_UPDATE)
        .about("Apply a guarded property edit to one note")
        .long_about(
            "Apply a guarded JSON envelope of prop.set, prop.unset, \
             prop.rename, prop.move, prop.set_yaml, prop.restore, \
             setting.set, and setting.clear \
             operations to one vault note's frontmatter. The envelope \
             carries a semantic fingerprint in `expect`; Bob re-reads the \
             note under the vault lock and replans against those fresh \
             bytes, preserving unrelated body and formatting edits. Use \
             --dry-run to preview the candidate, fingerprints, inverse ops, \
             frontmatter diff, and setting effects without writing. Owned \
             keys stay read-only here. See docs/props.md for the envelope, \
             operations, undo, and JSON.",
        )
        .after_help(
            "Examples:\n  \
             bob props update notes/groceries.md -i update.json -f json\n  \
             bob props update eat/ninja -i - -d -f json < preview.json\n  \
             bob props update 2026/20261122 -i undo.json\n\n\
             Environment:\n  \
             BOB_DIR                   Bob vault root when --bob-dir is omitted\n  \
             BOB_VAULT_SYNC_LOCK_FILE  Maintenance lock; defaults to ${XDG_RUNTIME_DIR:-/tmp}/bob_sync.lock\n  \
             NO_COLOR                  Disable colored output\n\n\
             Exit codes:\n  \
             0  previewed or updated\n  \
             1  vault I/O, lock busy, or stale guarded write\n  \
             2  usage, invalid envelope, or refused operation\n\n\
             See docs/props.md for the envelope, operations, Undo, and JSON.",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(input_arg())
        .arg(note_arg())
}

/// Run `bob props show`.
pub(crate) fn run_show(args: Vec<OsString>) -> i32 {
    let mut command = show_command();
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(COMMAND_SHOW)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error, COMMAND_SHOW),
    };
    execute_show(&matches)
}

/// Run `bob props update`.
pub(crate) fn run_update(args: Vec<OsString>) -> i32 {
    let mut command = update_command();
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(COMMAND_UPDATE)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error, COMMAND_UPDATE),
    };
    execute_update(&matches)
}

fn print_clap_error(error: clap::Error, command: &str) -> i32 {
    let exit_code = error.exit_code();
    if let Err(print_error) = error.print()
        && print_error.kind() != io::ErrorKind::BrokenPipe
    {
        eprintln!(
            "{command}: failed to print command-line error: {print_error}"
        );
    }
    exit_code
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
        .help("Preview the plan without writing files")
}

fn format_arg() -> Arg {
    Arg::new("format")
        .long("format")
        .short('f')
        .value_name("FORMAT")
        .value_parser(PossibleValuesParser::new([
            PossibleValue::new("text").alias("human"),
            PossibleValue::new("json"),
        ]))
        .default_value("text")
        .help("Output format: text or json")
}

fn help_arg() -> Arg {
    Arg::new("help")
        .long("help")
        .short('h')
        .action(ArgAction::Help)
        .help("Show help")
}

fn input_arg() -> Arg {
    Arg::new("input")
        .long("input")
        .short('i')
        .value_name("FILE")
        .required(true)
        .value_parser(OsStringValueParser::new())
        .help("Guarded JSON envelope file, or - for stdin")
}

fn note_arg() -> Arg {
    Arg::new("note").value_name("NOTE").required(true).help(
        "Vault-relative Markdown path (`.md` optional) or absolute \
             Markdown path inside the vault",
    )
}

fn format_from(matches: &ArgMatches) -> OutputFormat {
    match matches
        .get_one::<String>("format")
        .map(String::as_str)
        .unwrap_or("text")
    {
        "json" => OutputFormat::Json,
        _ => OutputFormat::Human,
    }
}

fn vault_from(matches: &ArgMatches) -> PathBuf {
    let vault = matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .unwrap_or_else(bob_env::bob_dir);
    lexical_normalize(&absolute_path(&vault))
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(path),
            Err(_) => path.to_path_buf(),
        }
    }
}

/// One typed CLI failure with its process exit code.
#[derive(Debug, Clone)]
struct CliError {
    code: String,
    message: String,
    hint: String,
    exit_code: i32,
    key: Option<String>,
    line: Option<usize>,
    op_index: Option<usize>,
}

impl CliError {
    /// Protocol and semantic refusals exit 2.
    fn refusal(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            hint: "See `bob props show --help` and docs/props.md for the \
                   note path, envelope, and ownership rules."
                .to_owned(),
            exit_code: 2,
            key: None,
            line: None,
            op_index: None,
        }
    }

    /// Vault I/O and lock failures exit 1.
    fn io(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            hint: "Check the vault path, note, lock, and file permissions."
                .to_owned(),
            exit_code: 1,
            key: None,
            line: None,
            op_index: None,
        }
    }

    fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }

    fn error_json(&self) -> Value {
        let mut error = serde_json::Map::new();
        error.insert("code".to_owned(), json!(self.code));
        error.insert("message".to_owned(), json!(self.message));
        error.insert("hint".to_owned(), json!(self.hint));
        if let Some(key) = &self.key {
            error.insert("key".to_owned(), json!(key));
        }
        if let Some(line) = self.line {
            error.insert("line".to_owned(), json!(line));
        }
        if let Some(index) = self.op_index {
            error.insert("op_index".to_owned(), json!(index));
        }
        Value::Object(error)
    }
}

impl From<PropsError> for CliError {
    fn from(error: PropsError) -> Self {
        Self {
            code: error.code,
            message: error.message,
            hint: error.hint,
            exit_code: 2,
            key: error.key,
            line: error.line,
            op_index: error.op_index,
        }
    }
}

fn lock_error(error: ob::LockWaitError) -> CliError {
    match error {
        ob::LockWaitError::Timeout { path } => CliError::io(
            "lock_busy",
            format!(
                "another Bob vault maintenance run is active (waited on {})",
                path.display()
            ),
        ),
        ob::LockWaitError::Open { path, error } => CliError::io(
            "io_error",
            format!("could not open lock file {}: {error}", path.display()),
        ),
        ob::LockWaitError::Acquire { path, error } => CliError::io(
            "io_error",
            format!("could not lock {}: {error}", path.display()),
        ),
    }
}

/// Resolve NOTE to an existing vault file plus its normalized report path.
///
/// NOTE is a vault-relative Markdown path with an optional `.md` suffix,
/// or an absolute Markdown path confined to the vault. Traversal, paths
/// outside the vault, non-Markdown targets, symlink escapes, and missing
/// notes are semantic refusals; unreadable vault roots are I/O failures.
fn resolve_target(
    vault: &Path,
    note: &str,
) -> Result<(PathBuf, String), CliError> {
    if note.is_empty() {
        return Err(CliError::refusal(
            "prop_bad_path",
            "note path must not be empty",
        ));
    }
    let raw = Path::new(note);
    let joined = if raw.is_absolute() {
        PathBuf::from(note)
    } else {
        let mut with_suffix = note.to_owned();
        if Path::new(&with_suffix).extension().is_none() {
            with_suffix.push_str(".md");
        }
        vault.join(with_suffix)
    };
    if joined.extension().and_then(|ext| ext.to_str()) != Some("md") {
        return Err(CliError::refusal(
            "prop_not_markdown",
            format!("note target must be a Markdown file: {note}"),
        )
        .with_hint(
            "Pass a vault-relative Markdown path or an absolute .md path \
             inside the vault.",
        ));
    }
    let vault_canonical = fs::canonicalize(vault).map_err(|error| {
        CliError::io(
            "vault_unavailable",
            format!("could not resolve vault {}: {error}", vault.display()),
        )
    })?;
    let lexical = lexical_normalize(&joined);
    let lexical_vault = lexical_normalize(vault);
    // Vault and NOTE may spell a symlinked ancestor differently (notably
    // /var and /private/var on macOS). Check the nearest existing parent
    // in that case, leaving file symlink checks and their errors below.
    if !lexical.starts_with(&lexical_vault)
        && !lexical
            .ancestors()
            .skip(1)
            .find_map(|ancestor| fs::canonicalize(ancestor).ok())
            .is_some_and(|ancestor| ancestor.starts_with(&vault_canonical))
    {
        return Err(CliError::refusal(
            "prop_traversal",
            format!("note path escapes the vault: {note}"),
        )
        .with_hint(
            "Pass a vault-relative Markdown path or an absolute .md path \
             inside the vault.",
        ));
    }
    match fs::symlink_metadata(&joined) {
        Ok(metadata)
            if metadata.is_file() || metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(CliError::refusal(
                "prop_not_markdown",
                format!("note target is not a file: {note}"),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(CliError::refusal(
                "prop_missing",
                format!("note does not exist: {note}"),
            )
            .with_hint(
                "props never creates notes; create the note first, then edit \
                 its properties.",
            ));
        }
        Err(error) => {
            return Err(CliError::io(
                "note_read_failed",
                format!("could not stat {}: {error}", joined.display()),
            ));
        }
    }
    let canonical = fs::canonicalize(&joined).map_err(|error| {
        CliError::io(
            "note_read_failed",
            format!("could not resolve {}: {error}", joined.display()),
        )
    })?;
    if !canonical.starts_with(&vault_canonical) {
        return Err(CliError::refusal(
            "prop_symlink_escape",
            format!("note target escapes the vault through a symlink: {note}"),
        ));
    }
    if canonical.extension().and_then(|ext| ext.to_str()) != Some("md") {
        return Err(CliError::refusal(
            "prop_not_markdown",
            format!("note target must be a Markdown file: {note}"),
        ));
    }
    let relative = canonical.strip_prefix(&vault_canonical).map_err(|_| {
        CliError::refusal(
            "prop_traversal",
            format!("note path escapes the vault: {note}"),
        )
    })?;
    Ok((vault_canonical.join(relative), slash_path(relative)))
}

/// Join path components without touching the filesystem.
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn slash_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Load the role context for one note's current bytes.
fn load_roles(
    vault: &Path,
    relative: &str,
    contents: &str,
) -> crate::native::props::roles::RoleContext {
    let records = RecordContext::discover(vault);
    resolve_note_context(vault, relative, contents, &records)
}

fn inspect_note(
    vault: &Path,
    relative: &str,
    contents: &str,
) -> (InspectContext, super::PropsInspection) {
    let mut context = InspectContext {
        contract: None,
        normalized_path: relative.to_owned(),
        roles: load_roles(vault, relative, contents),
    };
    load_contract(vault, &mut context);
    let inspection = inspect(contents, &context);
    (context, inspection)
}

fn load_contract(vault: &Path, context: &mut InspectContext) {
    if !super::contract::settings::for_kind(context.roles.note_kind).is_empty()
    {
        context.contract = Some(super::contract::Context::load(vault));
    }
}

fn execute_show(matches: &ArgMatches) -> i32 {
    let format = format_from(matches);
    let vault = vault_from(matches);
    let note = matches.get_one::<String>("note").expect("required note");
    let (absolute, relative) = match resolve_target(&vault, note) {
        Ok(target) => target,
        Err(error) => return report_show_error(COMMAND_SHOW, format, &error),
    };
    let contents = match fs::read_to_string(&absolute) {
        Ok(contents) => contents,
        Err(error) => {
            return report_show_error(
                COMMAND_SHOW,
                format,
                &CliError::io(
                    "note_read_failed",
                    format!("could not read {}: {error}", absolute.display()),
                ),
            );
        }
    };
    let (_, inspection) = inspect_note(&vault, &relative, &contents);
    if format == OutputFormat::Json {
        println!(
            "{}",
            serde_json::to_value(&inspection.report).unwrap_or(Value::Null)
        );
    } else {
        print!("{}", show_human(&inspection.report, &Styler::detect()));
    }
    0
}

fn report_show_error(
    command: &str,
    format: OutputFormat,
    error: &CliError,
) -> i32 {
    if format == OutputFormat::Json {
        println!(
            "{}",
            json!({
                "ok": false,
                "schema_version": super::show::SCHEMA_VERSION,
                "error": error.error_json(),
            })
        );
    } else {
        eprintln!("{command}: {}", error.message);
    }
    error.exit_code
}

fn display_value(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::String(text) => text.clone(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::Array(_) | Value::Object(_) => {
            serde_json::to_string(value).unwrap_or_else(|_| "…".to_owned())
        }
    }
}

fn show_human(report: &ShowReport, styler: &Styler) -> String {
    use crate::native::style::pad_right;

    let mut lines = Vec::new();
    lines.push(format!(
        "{}  {} · {}",
        styler.bold("Properties"),
        report.path,
        report.note_kind,
    ));
    lines.push(format!(
        "  {}",
        styler.dim(&match &report.fingerprint {
            Some(fingerprint) => format!("fingerprint {fingerprint}"),
            None => "fingerprint unavailable".to_owned(),
        })
    ));
    lines.push(format!("  frontmatter {}", frontmatter_summary(report)));
    if report.properties.is_empty() {
        lines.push("  (no properties)".to_owned());
    } else {
        let key_width = report
            .properties
            .iter()
            .map(|property| property.key.chars().count())
            .max()
            .unwrap_or(0)
            .min(32);
        for property in &report.properties {
            let role = property
                .role
                .map(|role| format!("{role:?}").to_lowercase())
                .unwrap_or_else(|| "unknown".to_owned());
            let mut row = format!(
                "  {}  {}  {}  {}",
                styler.cyan(&pad_right(&property.key, key_width)),
                display_value(&property.value),
                format!("{:?}", property.kind).to_lowercase(),
                role,
            );
            if let Some(owner) = &property.owner {
                row.push_str(&format!("  ({owner})"));
            }
            if property.duplicate {
                row.push_str("  [duplicate]");
            }
            if property.opaque {
                row.push_str("  [opaque]");
            }
            lines.push(row);
            if let Some(diagnostic) = &property.diagnostic {
                lines.push(format!("    {}", styler.dim(diagnostic)));
            }
        }
    }
    if let Some(contract) = &report.contract {
        lines.push(String::new());
        lines.push(format!("{}  {}", styler.bold("Settings"), contract.kind));
        let label_width = contract
            .settings
            .iter()
            .map(|setting| setting.label.chars().count())
            .max()
            .unwrap_or(0)
            .min(16);
        for setting in &contract.settings {
            let value = if setting.disabled {
                format!("{}  (disabled)", setting.effective.label)
            } else {
                setting.effective.label.clone()
            };
            let row = format!(
                "  {}  {}",
                styler.cyan(&crate::native::style::pad_right(
                    setting.label,
                    label_width
                )),
                value,
            );
            if setting.declared {
                lines.push(row);
            } else {
                lines.push(styler.dim(&row));
            }
            for issue in &setting.issues {
                lines.push(format!(
                    "    {} {}",
                    styler.yellow(&issue.code),
                    issue.message
                ));
            }
        }
        if let Some(prj) = &contract.prj {
            if let Some(line) = &prj.line {
                lines.push(format!(
                    "  {}  {line}",
                    styler.cyan(&crate::native::style::pad_right(
                        "Goal",
                        label_width,
                    ))
                ));
            }
        }
    }
    for diagnostic in &report.ownership_diagnostics {
        lines.push(format!("  {} {diagnostic}", styler.yellow("warning:")));
    }
    if let Some(error) = &report.frontmatter.error {
        lines.push(format!("  {} {}", styler.red("error:"), error.message));
        lines.push(format!("    {}", styler.dim(&error.hint)));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn frontmatter_summary(report: &ShowReport) -> String {
    let state = format!("{:?}", report.frontmatter.state).to_lowercase();
    match (report.frontmatter.start_line, report.frontmatter.end_line) {
        (Some(start), Some(end)) => format!("{state} (lines {start}–{end})"),
        _ => state,
    }
}

fn execute_update(matches: &ArgMatches) -> i32 {
    let format = format_from(matches);
    let vault = vault_from(matches);
    let note = matches
        .get_one::<String>("note")
        .expect("required note")
        .clone();
    let dry_run = matches.get_flag("dry-run");
    let envelope_text = match read_input(matches) {
        Ok(text) => text,
        Err(error) => {
            return report_update_error(
                COMMAND_UPDATE,
                format,
                None,
                dry_run,
                &error,
            );
        }
    };
    let envelope = match parse_envelope(&envelope_text) {
        Ok(envelope) => envelope,
        Err(error) => {
            return report_update_error(
                COMMAND_UPDATE,
                format,
                None,
                dry_run,
                &CliError::from(error),
            );
        }
    };
    let (absolute, relative) = match resolve_target(&vault, &note) {
        Ok(target) => target,
        Err(error) => {
            return report_update_error(
                COMMAND_UPDATE,
                format,
                None,
                dry_run,
                &error,
            );
        }
    };
    let current = match fs::read_to_string(&absolute) {
        Ok(current) => current,
        Err(error) => {
            return report_update_error(
                COMMAND_UPDATE,
                format,
                Some(relative.clone()),
                dry_run,
                &CliError::io(
                    "note_read_failed",
                    format!("could not read {}: {error}", absolute.display()),
                ),
            );
        }
    };
    let (context, _) = inspect_note(&vault, &relative, &current);
    let preview = match plan(&current, &context, &envelope) {
        Ok(preview) => preview,
        Err(error) => {
            return report_update_error(
                COMMAND_UPDATE,
                format,
                Some(relative.clone()),
                dry_run,
                &CliError::from(error),
            );
        }
    };
    if dry_run || !preview.changed {
        print_update_report(
            format,
            &relative,
            dry_run,
            &preview.candidate,
            &current,
            &preview,
        );
        return 0;
    }

    let _lock =
        match ob::acquire_lock_waiting(LOCK_WAIT, || {}).map_err(lock_error) {
            Ok(lock) => lock,
            Err(error) => {
                return report_update_error(
                    COMMAND_UPDATE,
                    format,
                    Some(relative.clone()),
                    dry_run,
                    &error,
                );
            }
        };
    // Re-resolve the target and ownership dependencies under the lock so a
    // concurrent rename, symlink swap, or album schema change refuses or
    // replans instead of publishing against stale context.
    let (absolute, relative) = match resolve_target(&vault, &note) {
        Ok(target) => target,
        Err(error) => {
            return report_update_error(
                COMMAND_UPDATE,
                format,
                None,
                dry_run,
                &error,
            );
        }
    };
    let fresh = match fs::read_to_string(&absolute) {
        Ok(fresh) => fresh,
        Err(_) => {
            return report_update_error(
                COMMAND_UPDATE,
                format,
                Some(relative.clone()),
                dry_run,
                &CliError::io(
                    "note_read_failed",
                    format!(
                        "note disappeared while waiting for the vault lock: {}",
                        absolute.display()
                    ),
                ),
            );
        }
    };
    let (fresh_context, _) = inspect_note(&vault, &relative, &fresh);
    let planned = match plan(&fresh, &fresh_context, &envelope) {
        Ok(planned) => planned,
        Err(error) => {
            return report_update_error(
                COMMAND_UPDATE,
                format,
                Some(relative.clone()),
                dry_run,
                &CliError::from(error),
            );
        }
    };
    if !planned.changed {
        print_update_report(
            format,
            &relative,
            dry_run,
            &planned.candidate,
            &fresh,
            &planned,
        );
        return 0;
    }
    if let Err(error) = atomic_write(&absolute, &planned.candidate) {
        return report_update_error(
            COMMAND_UPDATE,
            format,
            Some(relative.clone()),
            dry_run,
            &CliError::io(
                "write_failed",
                format!("write {}: {error}", absolute.display()),
            ),
        );
    }
    print_update_report(
        format,
        &relative,
        false,
        &planned.candidate,
        &fresh,
        &planned,
    );
    0
}

fn report_update_error(
    command: &str,
    format: OutputFormat,
    path: Option<String>,
    dry_run: bool,
    error: &CliError,
) -> i32 {
    if format == OutputFormat::Json {
        let mut payload = serde_json::Map::new();
        payload.insert("ok".to_owned(), json!(false));
        payload.insert(
            "schema_version".to_owned(),
            json!(super::show::SCHEMA_VERSION),
        );
        if let Some(path) = path {
            payload.insert("path".to_owned(), json!(path));
        }
        payload.insert("dry_run".to_owned(), json!(dry_run));
        payload.insert("changed".to_owned(), json!(false));
        payload.insert("error".to_owned(), error.error_json());
        println!("{}", Value::Object(payload));
    } else {
        eprintln!("{command}: {}", error.message);
    }
    error.exit_code
}

fn read_input(matches: &ArgMatches) -> Result<String, CliError> {
    let path = matches
        .get_one::<OsString>("input")
        .expect("required input");
    if path == OsStr::new("-") {
        let mut text = String::new();
        io::stdin().read_to_string(&mut text).map_err(|error| {
            CliError::io("input_read_failed", format!("read stdin: {error}"))
        })?;
        return Ok(text);
    }
    fs::read_to_string(Path::new(path)).map_err(|error| {
        CliError::io(
            "input_read_failed",
            format!("could not read {}: {error}", Path::new(path).display()),
        )
    })
}

struct PlannedView<'a> {
    fingerprint_before: &'a str,
    fingerprint_after: &'a str,
    changed: bool,
    inverse_ops: &'a [Op],
    report: &'a ShowReport,
}

fn print_update_report(
    format: OutputFormat,
    relative: &str,
    dry_run: bool,
    candidate: &str,
    before: &str,
    preview: &super::plan::PlanOutcome,
) {
    let view = PlannedView {
        fingerprint_before: &preview.fingerprint_before,
        fingerprint_after: &preview.fingerprint_after,
        changed: preview.changed,
        inverse_ops: &preview.inverse_ops,
        report: &preview.report,
    };
    let changes = frontmatter_diff(before, candidate);
    if format == OutputFormat::Json {
        println!(
            "{}",
            json!({
                "ok": true,
                "schema_version": super::show::SCHEMA_VERSION,
                "path": relative,
                "dry_run": dry_run,
                "changed": view.changed,
                "fingerprint_before": view.fingerprint_before,
                "fingerprint_after": view.fingerprint_after,
                "inverse_ops": view.inverse_ops.iter().map(Op::to_json).collect::<Vec<_>>(),
                "changes": changes,
                "warnings": preview.warnings,
                "effects": preview.effects,
                "properties": serde_json::to_value(&view.report.properties).unwrap_or(Value::Null),
            })
        );
    } else {
        for warning in &preview.warnings {
            println!("Warning [{}]: {}", warning.code, warning.message);
        }
        print!(
            "{}",
            update_human(
                relative,
                dry_run,
                &view,
                &changes,
                &preview.effects,
                &Styler::detect(),
            )
        );
    }
}

fn update_human(
    relative: &str,
    dry_run: bool,
    view: &PlannedView<'_>,
    changes: &[String],
    effects: &[super::contract::effects::Effect],
    styler: &Styler,
) -> String {
    let action = if dry_run {
        "Would update"
    } else if view.changed {
        "Updated"
    } else {
        "Unchanged"
    };
    let mut lines = vec![format!("{action}  {relative}")];
    lines.push(format!(
        "  {}",
        styler.dim(&format!(
            "fingerprint {} → {}",
            view.fingerprint_before, view.fingerprint_after
        ))
    ));
    if changes.is_empty() {
        lines.push("  (no frontmatter changes)".to_owned());
    } else {
        for change in changes {
            lines.push(format!("  {change}"));
        }
    }
    for effect in effects {
        lines.push(format!("  {}", styler.dim(&effect.message)));
    }
    lines.push(format!(
        "  {} inverse op(s) available for undo",
        view.inverse_ops.len()
    ));
    lines.push(String::new());
    lines.join("\n")
}

/// Minimal frontmatter-only line diff between two note images.
///
/// Both images are split into lines; the shared prefix and suffix are
/// skipped and the middle is reported as removed (`- `) then added
/// (`+ `) lines. Unchanged images produce an empty diff.
fn frontmatter_diff(before: &str, after: &str) -> Vec<String> {
    let old = frontmatter_text(before);
    let new = frontmatter_text(after);
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let mut prefix = 0;
    while prefix < old_lines.len()
        && prefix < new_lines.len()
        && old_lines[prefix] == new_lines[prefix]
    {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < old_lines.len() - prefix
        && suffix < new_lines.len() - prefix
        && old_lines[old_lines.len() - 1 - suffix]
            == new_lines[new_lines.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let mut changes = Vec::new();
    for line in &old_lines[prefix..old_lines.len() - suffix] {
        changes.push(format!("- {line}"));
    }
    for line in &new_lines[prefix..new_lines.len() - suffix] {
        changes.push(format!("+ {line}"));
    }
    changes
}

/// The frontmatter block (delimiters included) or an empty string.
fn frontmatter_text(contents: &str) -> &str {
    let document = PropsDocument::parse(contents);
    match document.frontmatter_range {
        Some(range) => &contents[range],
        None => "",
    }
}

fn atomic_write(target: &Path, contents: &str) -> io::Result<()> {
    use io::Write;
    let mut temp_name = OsString::from(".");
    temp_name.push(target.file_name().unwrap_or_else(|| OsStr::new("note.md")));
    temp_name.push(format!(".{}.tmp", std::process::id()));
    let temp = target.with_file_name(temp_name);
    let _ = fs::remove_file(&temp);
    let mut file = fs::File::create(&temp)?;
    let result = file
        .write_all(contents.as_bytes())
        .and_then(|()| file.sync_all());
    if let Err(error) = result {
        drop(file);
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    drop(file);
    match fs::rename(&temp, target) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temp);
            Err(error)
        }
    }
}

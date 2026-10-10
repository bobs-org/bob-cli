//! SASE-only listen launcher contract and runner (`--listen`).
//!
//! The setting comes from `BOB_HIGHLIGHTS_LISTEN_COMMAND`, otherwise from
//! `highlights.listen_command`. It must be a direct `sase listen render`
//! invocation (a direct `sase-listen render` invocation migrates
//! automatically) with arguments that write MP3 audio to `{audio}`. Bob
//! passes `{target}`, `{pdf}`, `{audio}`, and `{title}` as single arguments
//! to `sase` itself, streams the child's output unchanged, and verifies the
//! MP3. No shell runs: wrappers, chains, pipelines, redirections, and shell
//! expansions are rejected before any fetch or render.
use super::*;
use crate::native::env as bob_env;

pub(super) const ENV_LISTEN_COMMAND: &str = "BOB_HIGHLIGHTS_LISTEN_COMMAND";

/// Canonical SASE render template shown in every remediation hint.
const CANONICAL_TEMPLATE: &str =
    "sase listen render {target} -e full -o {audio}";
/// Hint printed when `--listen` is requested without a configured command.
const NOT_CONFIGURED_HINT: &str =
    "add `listen_command: sase listen render {target} -e full -o {audio}` under `highlights:`";
/// Hint printed when the listen command itself fails: nothing was written.
const LISTEN_FAILED_HINT: &str = "nothing was written to the vault; rerun the same command once the listen error above is fixed";
/// Hint printed when `sase` itself cannot be launched.
const SASE_LAUNCH_HINT: &str = "check that SASE is installed with its listen plugin (`sase listen --help`); nothing was written to the vault";
/// Hint printed when a placeholder is wrapped in quotes.
const QUOTED_PLACEHOLDER_HINT: &str =
    "bob passes placeholder values as single arguments itself; remove the quotes";
/// Shared migration explanation: the setting is SASE render arguments.
const MIGRATION_EXPLANATION: &str =
    "settings now describe SASE render arguments rather than shell programs";
/// Canonical remediation for every rejected template.
const MIGRATION_HINT: &str = "use `listen_command: sase listen render {target} -e full -o {audio}` under `highlights:` (a direct `sase-listen render ...` setting migrates automatically; convert wrappers, chains, pipelines, redirections, and shell expansions to plain arguments)";

const KNOWN_PLACEHOLDERS: &[&str] = &["target", "pdf", "audio", "title"];

/// Values substituted into a [`ListenCommand`] template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ListenValues {
    pub(super) target: String,
    pub(super) pdf: PathBuf,
    pub(super) audio: PathBuf,
    pub(super) title: String,
}

/// Where a listen run sends its progress and child output. CLI
/// `ref create -L` inherits the terminal (`Print`); background ingest
/// (`Quiet`) sends everything to stderr/log handling and never touches
/// JSON stdout, so a noisy fake listener cannot corrupt it. Quiet mode
/// also avoids buffering unlimited child output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum ListenOutput {
    #[default]
    Print,
    Quiet,
}

/// A validated-or-not SASE render invocation template.
///
/// The raw trimmed setting is kept verbatim. Only a direct `sase listen
/// render ...` invocation is accepted; a direct `sase-listen render ...`
/// invocation is normalized to `sase listen render ...` in memory (token for
/// token, never by substring rewrite, so runtime data containing
/// `sase-listen` stays intact). The file on disk is never rewritten.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ListenCommand {
    command: String,
}

/// How a listen run can fail. Every variant carries the user-facing
/// `message` and its follow-up `hint`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ListenError {
    NotConfigured { message: String, hint: String },
    Invalid { message: String, hint: String },
    Failed { message: String, hint: String },
    Interrupted { message: String, hint: String },
    NoAudio { message: String, hint: String },
}

impl ListenError {
    pub(super) fn message(&self) -> &str {
        match self {
            Self::NotConfigured { message, .. }
            | Self::Invalid { message, .. }
            | Self::Failed { message, .. }
            | Self::Interrupted { message, .. }
            | Self::NoAudio { message, .. } => message,
        }
    }

    pub(super) fn hint(&self) -> &str {
        match self {
            Self::NotConfigured { hint, .. }
            | Self::Invalid { hint, .. }
            | Self::Failed { hint, .. }
            | Self::Interrupted { hint, .. }
            | Self::NoAudio { hint, .. } => hint,
        }
    }

    pub(super) fn into_command_error(self) -> CommandError {
        let code = self.exit_code();
        CommandError::new(format!("{}\nhint: {}", self.message(), self.hint()))
            .with_exit_code(code)
    }

    /// Process exit code for a listen failure: an interrupted run exits
    /// 130 like the signal, everything else exits 1.
    pub(super) fn exit_code(&self) -> i32 {
        match self {
            Self::Interrupted { .. } => 130,
            _ => 1,
        }
    }
}

/// Resolve the configured listen command and validate its template in
/// one step. `--listen` callers run this before any fetch or render so
/// an unconfigured or invalid command fails before vault work starts.
pub(super) fn require_validated(
) -> std::result::Result<ListenCommand, ListenError> {
    let command = ListenCommand::require()?;
    command.validate()?;
    Ok(command)
}

/// One planned `--listen` episode: the scratch MP3 the listen command
/// writes, the intake `<target>.mp3` it is bound to, and the mirrored
/// library audio that must not already exist.
#[derive(Debug, Clone)]
pub(super) struct ListenFlow {
    pub(super) scratch_audio: PathBuf,
    pub(super) dest: PathBuf,
    pub(super) library_dest: Option<PathBuf>,
}

/// Plan the episode destination for a normal (non-attach) `--listen`
/// run. Refuses when the mirrored library audio already exists; an
/// existing intake audio is compared after the listen run, when both
/// byte strings exist.
pub(super) fn plan_listen_flow(
    target_plan: &TargetPlan,
    scratch: &ScratchDir,
    stem: &str,
) -> Result<ListenFlow> {
    let dest = target_plan.target.with_extension("mp3");
    let library_dest = match &target_plan.workflow {
        TargetWorkflow::Intake {
            library_destination,
        } => Some(library_destination.with_extension("mp3")),
        TargetWorkflow::Library | TargetWorkflow::External => None,
    };
    if let Some(library_dest) = &library_dest
        && library_dest.exists()
    {
        return Err(CommandError::new(format!(
            "refusing to create {} because the library destination already exists: {}; remove or rename the archived copy before recreating it (bob ref scan would refuse to move the new audio over it)",
            dest.display(),
            library_dest.display()
        )));
    }
    Ok(ListenFlow {
        scratch_audio: scratch.path().join(format!("{stem}.mp3")),
        dest,
        library_dest,
    })
}

/// Install the episode after the listen command produced it: re-check
/// the mirrored library audio (the vault may have changed during a
/// long listen), refuse a different existing intake audio unless
/// `--force` is given, and `atomic_copy` the episode. Identical bytes
/// count as a reuse and skip the copy.
pub(super) fn install_listen_audio(
    flow: &ListenFlow,
    force: bool,
) -> Result<Option<PathBuf>> {
    if let Some(library_dest) = &flow.library_dest
        && library_dest.exists()
    {
        return Err(CommandError::new(format!(
            "refusing to create {} because the library destination already exists: {}; remove or rename the archived copy before recreating it (bob ref scan would refuse to move the new audio over it)",
            flow.dest.display(),
            library_dest.display()
        )));
    }
    if flow.dest.exists() {
        if super::companion::files_have_identical_bytes(
            &flow.scratch_audio,
            &flow.dest,
        )? {
            return Ok(None);
        }
        if !force {
            return Err(CommandError::new(format!(
                "target audio already exists: {}; pass --force to overwrite it",
                flow.dest.display()
            )));
        }
    }
    atomic_copy(&flow.scratch_audio, &flow.dest)?;
    Ok(Some(flow.dest.clone()))
}

/// Report a failure that happened after the listen command produced
/// audio: keep the scratch directory, print the kept audio path, and
/// return the failure with its recovery hint.
pub(super) fn post_listen_error(
    scratch: &mut ScratchDir,
    scratch_audio: &Path,
    message: String,
    hint: String,
) -> CommandError {
    scratch.keep();
    println!("kept: {}", scratch_audio.display());
    CommandError::new(format!("{message}\nhint: {hint}"))
}

/// Quiet variant for background ingest: keeps the scratch directory,
/// reports the kept path to stderr (never stdout, so JSON stays clean),
/// and returns the failure with its recovery hint.
pub(super) fn post_listen_error_quiet(
    scratch: &mut ScratchDir,
    scratch_audio: &Path,
    message: String,
    hint: String,
    progress: Option<&dyn Fn(&str)>,
) -> CommandError {
    scratch.keep();
    let line = format!("kept: {}", scratch_audio.display());
    eprintln!("{line}");
    if let Some(report) = progress {
        report(&line);
    }
    CommandError::new(format!("{message}\nhint: {hint}"))
}

/// The dry-run line for a listen run that never executes. Display uses the
/// normalized `sase listen render ...` argv (shell quoting is display-only).
pub(super) fn would_run_line(
    command: &ListenCommand,
    values: &ListenValues,
) -> String {
    match command.expanded_argv(values) {
        Ok(argv) => {
            format!("listen: would-run {}", format_argv_for_display(&argv))
        }
        Err(_) => format!("listen: would-run {}", command.command),
    }
}

impl fmt::Display for ListenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}\nhint: {}", self.message(), self.hint())
    }
}

impl StdError for ListenError {}

fn invalid(message: String, hint: impl Into<String>) -> ListenError {
    ListenError::Invalid {
        message,
        hint: hint.into(),
    }
}

impl ListenCommand {
    pub(super) fn display(&self) -> &str {
        &self.command
    }

    /// Resolve the configured command: the environment override wins, then
    /// the config file. Blank means unset. A broken config file is an error.
    pub(super) fn resolve() -> Result<Option<Self>> {
        if let Some(command) = bob_env::var_os(ENV_LISTEN_COMMAND) {
            let command = command.to_string_lossy().trim().to_string();
            if command.is_empty() {
                return Ok(None);
            }
            return Ok(Some(Self { command }));
        }

        let config =
            bob_config::load_highlights_config(&bob_config::config_path())
                .map_err(config_error)?;
        Ok(config.listen_command().map(|command| Self {
            command: command.to_string(),
        }))
    }

    /// Resolve, or fail with the unconfigured `--listen` error.
    pub(super) fn require() -> std::result::Result<Self, ListenError> {
        match Self::resolve() {
            Ok(Some(command)) => Ok(command),
            Ok(None) => {
                let path = bob_config::config_path();
                Err(ListenError::NotConfigured {
                    message: format!(
                        "--listen needs highlights.listen_command in {} (or {ENV_LISTEN_COMMAND})",
                        path.display()
                    ),
                    hint: NOT_CONFIGURED_HINT.to_string(),
                })
            }
            Err(error) => Err(invalid(
                error.to_string(),
                "fix the config file, then rerun the command",
            )),
        }
    }

    /// Check the SASE-only contract: a direct `sase listen render`
    /// invocation (or a direct `sase-listen render` legacy invocation that
    /// migrates), `{audio}` plus `{target}` or `{pdf}`, known placeholders
    /// only, and no quoting around placeholders (bob passes values as single
    /// arguments itself).
    pub(super) fn validate(&self) -> std::result::Result<(), ListenError> {
        if let Some(name) = quoted_placeholder(&self.command) {
            return Err(invalid(
                format!(
                    "highlights.listen_command quotes placeholder {{{name}}}; \
                     bob passes placeholder values as single arguments itself"
                ),
                QUOTED_PLACEHOLDER_HINT,
            ));
        }
        let render_args = self.parse_render_template()?;
        let placeholders = find_placeholders_in_args(&render_args);
        for name in &placeholders {
            if !KNOWN_PLACEHOLDERS.contains(&name.as_str()) {
                return Err(invalid(
                    format!(
                        "unknown placeholder {{{name}}} in \
                         highlights.listen_command (expected {{target}}, \
                         {{pdf}}, {{audio}}, or {{title}})"
                    ),
                    format!(
                        "use only {{target}}, {{pdf}}, {{audio}}, and {{title}}; {MIGRATION_EXPLANATION}: \
                         `{CANONICAL_TEMPLATE}`"
                    ),
                ));
            }
        }
        if !placeholders.iter().any(|name| name == "audio") {
            return Err(invalid(
                "highlights.listen_command must write MP3 audio to {audio}"
                    .to_string(),
                format!(
                    "add {{audio}} to the command, for example: {CANONICAL_TEMPLATE}"
                ),
            ));
        }
        if !placeholders
            .iter()
            .any(|name| name == "target" || name == "pdf")
        {
            return Err(invalid(
                "highlights.listen_command must contain {target} or {pdf}"
                    .to_string(),
                format!(
                    "add the narration source, for example: {CANONICAL_TEMPLATE}"
                ),
            ));
        }
        Ok(())
    }

    /// Parse the raw setting into SASE render argument templates (the words
    /// after `sase listen render`, placeholders still intact).
    ///
    /// Only `sase listen render ...` is accepted. A direct `sase-listen
    /// render ...` invocation migrates automatically by token (never by
    /// substring, so data containing `sase-listen` is untouched). Anything
    /// else — other executables, paths, `env`/`command`/shell wrappers,
    /// chains, pipelines, redirections, substitutions — fails with the
    /// canonical remediation before any fetch or render.
    fn parse_render_template(
        &self,
    ) -> std::result::Result<Vec<String>, ListenError> {
        let raw = &self.command;
        if raw.contains('\0') {
            return Err(invalid(
                format!(
                    "highlights.listen_command contains a NUL byte; \
                     {MIGRATION_EXPLANATION}: `{CANONICAL_TEMPLATE}`"
                ),
                MIGRATION_HINT,
            ));
        }
        if let Some(kind) = find_unquoted_shell_operator(raw) {
            return Err(invalid(
                format!(
                    "unsupported highlights.listen_command ({kind}); \
                     {MIGRATION_EXPLANATION}: `{CANONICAL_TEMPLATE}`"
                ),
                MIGRATION_HINT,
            ));
        }
        let tokens = shlex::split(raw).ok_or_else(|| {
            invalid(
                format!(
                    "highlights.listen_command has malformed quoting; \
                     {MIGRATION_EXPLANATION}: `{CANONICAL_TEMPLATE}`"
                ),
                MIGRATION_HINT,
            )
        })?;
        if tokens.is_empty() {
            return Err(invalid(
                format!(
                    "unsupported highlights.listen_command (empty); \
                     {MIGRATION_EXPLANATION}: `{CANONICAL_TEMPLATE}`"
                ),
                MIGRATION_HINT,
            ));
        }
        if tokens.len() >= 3
            && tokens[0] == "sase"
            && tokens[1] == "listen"
            && tokens[2] == "render"
        {
            return Ok(tokens[3..].to_vec());
        }
        // Limited legacy migration: only a direct `sase-listen render`
        // invocation (complete tokens) normalizes to `sase listen render`.
        if tokens.len() >= 2
            && tokens[0] == "sase-listen"
            && tokens[1] == "render"
        {
            return Ok(tokens[2..].to_vec());
        }
        let reason = if tokens[0].contains('/') || tokens[0].contains('\\') {
            "executable paths are not supported"
        } else if is_env_assignment(&tokens[0]) {
            "leading environment assignments are not supported"
        } else if is_launcher_wrapper(&tokens[0]) {
            "wrappers (`env`, `command`, shells, `script`, `sudo`, ...) are not supported"
        } else {
            "only a direct `sase listen render ...` invocation is supported"
        };
        Err(invalid(
            format!(
                "unsupported highlights.listen_command ({reason}: `{raw}`); \
                 {MIGRATION_EXPLANATION}: `{CANONICAL_TEMPLATE}`"
            ),
            MIGRATION_HINT,
        ))
    }

    /// Expand placeholder templates into raw argument values (no shell
    /// quoting: each value stays data in the same argument).
    fn expand_render_args(
        &self,
        values: &ListenValues,
    ) -> std::result::Result<Vec<String>, ListenError> {
        Ok(self
            .parse_render_template()?
            .iter()
            .map(|token| expand_arg_template(token, values))
            .collect())
    }

    /// Full `sase listen render ...` argv for execution (raw values).
    fn expanded_argv(
        &self,
        values: &ListenValues,
    ) -> std::result::Result<Vec<String>, ListenError> {
        let mut argv = vec![
            "sase".to_string(),
            "listen".to_string(),
            "render".to_string(),
        ];
        argv.extend(self.expand_render_args(values)?);
        Ok(argv)
    }

    /// Normalized template for doctor (unexpanded, `sase listen render`
    /// prefix, placeholders bare). Falls back to the raw setting when the
    /// template is invalid (doctor reports the validation error instead).
    fn normalized_template_display(&self) -> String {
        let Ok(render_args) = self.parse_render_template() else {
            return self.command.clone();
        };
        let mut out = String::from("sase listen render");
        for token in &render_args {
            out.push(' ');
            out.push_str(&format_template_token_for_display(token));
        }
        out
    }
}

/// Expand `{target}`/`{pdf}`/`{audio}`/`{title}` inside one argument-template
/// token into raw values. Unknown `{...}` shapes are left literal (they were
/// already rejected when lowercase-unknown); runtime values are never
/// rescanned as shell programs.
fn expand_arg_template(token: &str, values: &ListenValues) -> String {
    let mut out = String::with_capacity(token.len());
    let mut cursor = 0;
    while cursor < token.len() {
        if token.as_bytes()[cursor] == b'{'
            && let Some((name, end)) = placeholder_at(token, cursor)
            && let Some(raw) = raw_placeholder_value(name, values)
        {
            out.push_str(&raw);
            cursor = end;
            continue;
        }
        let character = token[cursor..].chars().next().unwrap_or('\0');
        if character == '\0' {
            break;
        }
        out.push(character);
        cursor += character.len_utf8();
    }
    out
}

fn raw_placeholder_value(name: &str, values: &ListenValues) -> Option<String> {
    match name {
        "target" => Some(values.target.clone()),
        "pdf" => Some(values.pdf.to_string_lossy().into_owned()),
        "audio" => Some(values.audio.to_string_lossy().into_owned()),
        "title" => Some(values.title.clone()),
        _ => None,
    }
}

/// Format one template token for the normalized doctor display: bare unless
/// it needs quoting for readability (spaces/quotes). Placeholders stay bare.
fn format_template_token_for_display(token: &str) -> String {
    if token.contains(' ')
        || token.contains('\t')
        || token.contains('\'')
        || token.contains('"')
    {
        shell_quote(token)
    } else {
        token.to_string()
    }
}

/// Format an expanded argv for the human-readable `listen: run ...` /
/// `listen: would-run ...` lines. Shell quoting is display-only.
fn format_argv_for_display(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| shell_quote(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

/// True when `word` looks like `NAME=value` (a leading environment
/// assignment, never a SASE launcher).
fn is_env_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// True for `env`/`command`/shell/`script`/`sudo`-style wrappers. These never
/// name the SASE launcher directly.
fn is_launcher_wrapper(word: &str) -> bool {
    matches!(
        word,
        "env"
            | "command"
            | "sh"
            | "bash"
            | "dash"
            | "zsh"
            | "fish"
            | "sudo"
            | "doas"
            | "su"
            | "nice"
            | "time"
            | "nohup"
            | "setsid"
            | "script"
            | "xargs"
            | "strace"
            | "gdb"
    )
}

/// Find unquoted shell syntax in the template: chains (`;`, `&`, newlines),
/// pipelines (`|`), redirections (`<`, `>`), groups (`(`, `)`), and
/// substitutions/expansions (`$`, backticks). Single-quoted data may contain
/// any punctuation; double-quoted `$`/backticks still expand in a shell, so
/// they are rejected. Backslash escapes the next character (literal data).
/// Unclosed quotes are left for the tokenizer to reject as malformed.
fn find_unquoted_shell_operator(template: &str) -> Option<&'static str> {
    let mut chars = template.chars();
    let mut in_single = false;
    let mut in_double = false;
    while let Some(c) = chars.next() {
        if in_single {
            if c == '\'' {
                in_single = false;
            }
            continue;
        }
        if in_double {
            if c == '\\' {
                let _ = chars.next();
                continue;
            }
            if c == '"' {
                in_double = false;
                continue;
            }
            if c == '$' || c == '`' {
                return Some("shell expansions");
            }
            continue;
        }
        if c == '\\' {
            let _ = chars.next();
            continue;
        }
        if c == '\'' {
            in_single = true;
            continue;
        }
        if c == '"' {
            in_double = true;
            continue;
        }
        match c {
            ';' => return Some("command chains (`;`)"),
            '&' => return Some("command chains (`&`)"),
            '|' => return Some("pipelines (`|`)"),
            '<' | '>' => return Some("redirections (`<`/`>`)"),
            '(' | ')' => return Some("subshells/command groups"),
            '\n' | '\r' => return Some("command chains (newline)"),
            '$' => return Some("shell expansions (`$`)"),
            '`' => return Some("shell substitutions (backticks)"),
            _ => {}
        }
    }
    None
}

/// Quote one value for `sh`: bare when every byte is shell-safe, otherwise
/// single-quoted with embedded quotes escaped.
pub(super) fn shell_quote(value: &str) -> String {
    let is_safe = !value.is_empty()
        && value.bytes().all(|byte| {
            matches!(
                byte,
                b'A'..=b'Z'
                    | b'a'..=b'z'
                    | b'0'..=b'9'
                    | b'_'
                    | b'.'
                    | b'/'
                    | b':'
                    | b'@'
                    | b'%'
                    | b'+'
                    | b'='
                    | b','
                    | b'-'
            )
        });
    if is_safe {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Parse the placeholder opening at `open` (`template[open] == '{'`).
/// Returns the name and the byte index just past the closing `}`.
fn placeholder_at(template: &str, open: usize) -> Option<(&str, usize)> {
    let rest = &template[open + 1..];
    let mut length = 0;
    for character in rest.chars() {
        if character == '}' {
            break;
        }
        if !(character.is_ascii_lowercase() || character == '_') {
            return None;
        }
        length += character.len_utf8();
    }
    if length == 0 || rest[length..].chars().next() != Some('}') {
        return None;
    }
    Some((&rest[..length], open + 1 + length + 1))
}

/// Collect placeholders from already-tokenized render arguments. Tokens are
/// scanned for `{name}`; `${...}` cannot occur (a `$` is rejected as shell
/// syntax before tokenization).
fn find_placeholders_in_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for arg in args {
        let mut cursor = 0;
        while cursor < arg.len() {
            if arg.as_bytes()[cursor] == b'{'
                && let Some((name, end)) = placeholder_at(arg, cursor)
            {
                out.push(name.to_string());
                cursor = end;
            } else {
                let character = arg[cursor..].chars().next().unwrap_or('\0');
                if character == '\0' {
                    break;
                }
                cursor += character.len_utf8();
            }
        }
    }
    out
}

/// Run the listen command with the values substituted in, streaming its
/// output unchanged (stdin, stdout, and stderr are inherited). Prints
/// `listen: run <expanded command>` first. On exit 0 the MP3 at `{audio}`
/// is verified and its path returned.
pub(super) fn run_listen(
    command: &ListenCommand,
    values: &ListenValues,
) -> std::result::Result<PathBuf, ListenError> {
    run_listen_with_output(command, values, ListenOutput::Print)
}

/// Same as [`run_listen`] with an explicit output policy. `Quiet` sends
/// the `listen: run …` line and the child's stdout/stderr to stderr so
/// background ingest never contaminates JSON stdout, and avoids
/// buffering unlimited child output by streaming the piped stdout to
/// stderr in bounded chunks.
pub(super) fn run_listen_with_output(
    command: &ListenCommand,
    values: &ListenValues,
    output: ListenOutput,
) -> std::result::Result<PathBuf, ListenError> {
    command.validate()?;
    let argv = command.expanded_argv(values)?;
    let display = format_argv_for_display(&argv);
    let expanded = display.clone();
    match output {
        ListenOutput::Print => {
            println!("listen: run {expanded}");
            {
                use std::io::Write as _;
                let _ = std::io::stdout().flush();
            }
        }
        ListenOutput::Quiet => {
            eprintln!("listen: run {expanded}");
        }
    }

    // Like `system(3)`: ignore SIGINT/SIGQUIT while the child runs so
    // Ctrl-C reaches the listen command's graceful stop, and reset both to
    // the default dispositions in the child.
    let previous_signals = parent_ignore_signals();
    // SASE-only launch: no shell. `argv` is `sase listen render ...` with raw
    // values as single arguments; quoting exists only in `display`.
    let mut child = process::Command::new("sase");
    child.args(&argv[1..]).stdin(Stdio::inherit());
    bob_env::inherit_overrides(&mut child);
    match output {
        ListenOutput::Print => {
            child.stdout(Stdio::inherit()).stderr(Stdio::inherit());
        }
        ListenOutput::Quiet => {
            // Pipe stdout so it can be forwarded to stderr without ever
            // touching stdout; stderr stays inherited.
            child.stdout(Stdio::piped()).stderr(Stdio::inherit());
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // SAFETY: `reset_child_signals` only calls the async-signal-safe
        // `signal` between `fork` and `exec`.
        unsafe {
            child.pre_exec(reset_child_signals);
        }
    }
    let status = match output {
        ListenOutput::Print => child.status(),
        ListenOutput::Quiet => {
            use std::io::Read as _;
            let mut spawned = child.spawn().map_err(|error| {
                parent_restore_signals(previous_signals);
                ListenError::Failed {
                    message: format!("run listen command {expanded}: {error}"),
                    hint: SASE_LAUNCH_HINT.to_string(),
                }
            })?;
            // Forward piped stdout to stderr in bounded 8 KiB chunks
            // (cap 1 MiB) so a noisy listener never contaminates JSON
            // stdout and never buffers unbounded output.
            if let Some(mut piped) = spawned.stdout.take() {
                let mut buf = [0u8; 8192];
                let mut total: usize = 0;
                loop {
                    match piped.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            total += n;
                            if total <= 1024 * 1024 {
                                use std::io::Write as _;
                                let _ = std::io::stderr().write_all(&buf[..n]);
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
            spawned.wait()
        }
    };
    // Restore signals after wait in both paths. The early spawn-error
    // path above already restored before returning.
    if matches!(output, ListenOutput::Print) {
        parent_restore_signals(previous_signals);
    } else {
        // Quiet spawn path has not restored yet unless it early-returned.
        // Restore now; the early-error return already restored.
        // Use a no-op restore guard: calling restore twice is safe because
        // it only installs dispositions.
        parent_restore_signals(previous_signals);
    }
    let status = status.map_err(|error| ListenError::Failed {
        message: format!("run listen command {expanded}: {error}"),
        hint: SASE_LAUNCH_HINT.to_string(),
    })?;

    if status.success() {
        if audio_is_mp3(&values.audio) {
            return Ok(values.audio.clone());
        }
        return Err(ListenError::NoAudio {
            message: format!(
                "listen command exited 0 but wrote no MP3 audio to {}",
                values.audio.display()
            ),
            hint: format!(
                "{LISTEN_FAILED_HINT}; make sure the command writes MP3 audio to {{audio}}"
            ),
        });
    }
    if status_interrupted(&status) {
        return Err(ListenError::Interrupted {
            message: "listen command interrupted".to_string(),
            hint: format!(
                "{LISTEN_FAILED_HINT}; rerun the same command to try again"
            ),
        });
    }
    Err(ListenError::Failed {
        message: format!(
            "listen command failed with {}",
            exit_status_label(&status)
        ),
        hint: LISTEN_FAILED_HINT.to_string(),
    })
}

fn status_interrupted(status: &process::ExitStatus) -> bool {
    if status.code() == Some(130) {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        status.signal() == Some(libc::SIGINT)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

#[cfg(unix)]
fn parent_ignore_signals() -> (libc::sighandler_t, libc::sighandler_t) {
    // SAFETY: installing SIG_IGN for SIGINT/SIGQUIT is async-signal-safe,
    // and the previous dispositions are restored after `wait` returns.
    unsafe {
        (
            libc::signal(libc::SIGINT, libc::SIG_IGN),
            libc::signal(libc::SIGQUIT, libc::SIG_IGN),
        )
    }
}

#[cfg(unix)]
fn parent_restore_signals(previous: (libc::sighandler_t, libc::sighandler_t)) {
    // SAFETY: restores the dispositions saved before spawning the child.
    unsafe {
        libc::signal(libc::SIGINT, previous.0);
        libc::signal(libc::SIGQUIT, previous.1);
    }
}

#[cfg(unix)]
fn reset_child_signals() -> std::io::Result<()> {
    // SAFETY: runs in the child between `fork` and `exec`, resetting the
    // dispositions the parent ignored so Ctrl-C reaches the listen command.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_DFL);
        libc::signal(libc::SIGQUIT, libc::SIG_DFL);
    }
    Ok(())
}

#[cfg(not(unix))]
fn parent_ignore_signals() {}

#[cfg(not(unix))]
fn parent_restore_signals(_previous: ()) {}

/// Check that `path` is a regular, non-empty file starting with an MP3
/// header: `ID3` or an MPEG frame sync (`0xFF`, second byte `& 0xE0`).
fn audio_is_mp3(path: &Path) -> bool {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return false,
    };
    if !metadata.is_file() || metadata.len() == 0 {
        return false;
    }
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return false,
    };
    use std::io::Read as _;
    let mut header = [0u8; 3];
    let mut read = 0;
    while read < header.len() {
        match file.read(&mut header[read..]) {
            Ok(0) => break,
            Ok(count) => read += count,
            Err(_) => return false,
        }
    }
    if read >= 3 && header.starts_with(b"ID3") {
        return true;
    }
    read >= 2 && header[0] == 0xFF && header[1] & 0xE0 == 0xE0
}

/// Non-fatal `doctor` row for the listen setting, printed after the pandoc
/// and web-clip rows. Problems warn; they never fail the vault doctor. An
/// `ok` row means the template is valid and the fixed `sase` launcher is
/// available — not that the listen plugin or its credentials work (use
/// `sase listen --help` to diagnose the plugin).
pub(super) fn append_listen_doctor_row(warnings: &mut Vec<String>) {
    let command = match ListenCommand::resolve() {
        Ok(command) => command,
        Err(error) => {
            println!("listen_command: warn ({error})");
            warnings.push(format!(
                "highlights listen command is misconfigured: {error}"
            ));
            return;
        }
    };
    let Some(command) = command else {
        println!("listen_command: none");
        return;
    };
    if let Err(error) = command.validate() {
        println!("listen_command: warn ({})", error.message());
        warnings.push(format!(
            "highlights listen command is invalid: {}; hint: {MIGRATION_HINT}",
            error.message()
        ));
        return;
    }
    let normalized = command.normalized_template_display();
    let program = OsString::from("sase");
    match shell_command_available(&program) {
        Ok(true) => {
            println!(
                "listen_command: ok ({normalized}; launcher available: sase; template valid — use `sase listen --help` to diagnose the listen plugin)"
            );
        }
        Ok(false) => {
            println!(
                "listen_command: warn ({normalized}; launcher not found: sase)"
            );
            warnings.push(format!(
                "highlights listen command launcher not found: sase ({normalized}); hint: {MIGRATION_HINT}"
            ));
        }
        Err(error) => {
            println!(
                "listen_command: warn ({normalized}; launcher check failed: {error})"
            );
            warnings.push(format!(
                "highlights listen command launcher check failed: {error}"
            ));
        }
    }
}

/// Find a placeholder wrapped in matching single or double quotes, if any.
fn quoted_placeholder(template: &str) -> Option<String> {
    let mut cursor = 0;
    while cursor < template.len() {
        let is_open = template.as_bytes()[cursor] == b'{'
            && (cursor == 0 || template.as_bytes()[cursor - 1] != b'$');
        if is_open && let Some((name, end)) = placeholder_at(template, cursor) {
            let before = template[..cursor].chars().next_back();
            let after = template[end..].chars().next();
            if matches!(
                (before, after),
                (Some('\''), Some('\'')) | (Some('"'), Some('"'))
            ) {
                return Some(name.to_string());
            }
            cursor = end;
        } else {
            let character = template[cursor..].chars().next().unwrap_or('\0');
            if character == '\0' {
                break;
            }
            cursor += character.len_utf8();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SCRATCH_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn test_command(template: &str) -> ListenCommand {
        ListenCommand {
            command: template.to_string(),
        }
    }

    fn test_values(scratch: &Path) -> ListenValues {
        ListenValues {
            target: "https://arxiv.org/abs/1706.03762".to_string(),
            pdf: scratch.join("attention_is_all_you_need.pdf"),
            audio: scratch.join("attention_is_all_you_need.mp3"),
            title: "Attention Is All You Need".to_string(),
        }
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let nonce = SCRATCH_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "bob-cli-listen-test-{name}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create listen test scratch dir");
        dir
    }

    fn write_executable(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, body).expect("write fake listen script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mut permissions = fs::metadata(&path)
                .expect("stat fake listen script")
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions)
                .expect("chmod fake listen script");
        }
        path
    }

    #[test]
    fn accepts_canonical_and_legacy_templates() {
        for template in [
            "sase listen render {target} -e full -o {audio}",
            "sase listen render {pdf} -e full -o {audio} --title {title}",
            "sase listen render {target} -e full -o {audio} --no-publish",
            "sase listen render --title 'static words' {target} -o {audio}",
            "sase-listen render {target} -e full -o {audio}",
            "sase-listen render {pdf} -e full -o {audio} --title {title} --no-publish",
        ] {
            test_command(template)
                .validate()
                .expect("template must validate");
        }
        // Legacy normalizes to the SASE launcher in memory.
        let legacy =
            test_command("sase-listen render {target} -e full -o {audio}");
        assert_eq!(
            legacy.normalized_template_display(),
            "sase listen render {target} -e full -o {audio}",
            "legacy must normalize to sase"
        );
        let canonical =
            test_command("sase listen render {target} -e full -o {audio}");
        assert_eq!(
            canonical.normalized_template_display(),
            "sase listen render {target} -e full -o {audio}",
        );
    }

    #[test]
    fn rejects_shell_wrappers_chains_and_other_programs() {
        for template in [
            "${HOME}/bin/render {target} -o {audio}",
            "render {target} --fmt {a,b} -o {audio}",
            "render {target} -o {audio}",
            "sase render {target} -o {audio}",
            "sase listen publish {target} -o {audio}",
            "sase listen {target} -o {audio}",
            "/usr/bin/sase listen render {target} -o {audio}",
            "./sase listen render {target} -o {audio}",
            "FOO=1 sase listen render {target} -o {audio}",
            "env sase listen render {target} -o {audio}",
            "command sase listen render {target} -o {audio}",
            "sh -c 'sase listen render {target} -o {audio}'",
            "bash -c 'sase listen render {target} -o {audio}'",
            "sase listen render {target} -o {audio}; rm -rf /",
            "sase listen render {target} -o {audio} && echo hi",
            "sase listen render {target} -o {audio} | cat",
            "sase listen render {target} -o {audio} > out.mp3",
            "sase listen render $HOME -o {audio} {target}",
            "sase listen render `evil` {target} -o {audio}",
            "sase listen render {target} -o {audio} ${HOME}",
            "sase-listen renderx {target} -o {audio}",
            "sase-listenx render {target} -o {audio}",
        ] {
            let error = test_command(template)
                .validate()
                .expect_err("template must fail");
            assert!(
                error.message().contains("sase listen render")
                    || error.message().contains("SASE render arguments"),
                "missing canonical remediation for {template:?}: {}",
                error.message()
            );
            assert!(
                error.hint().contains("sase listen render {target}"),
                "missing canonical hint for {template:?}: {}",
                error.hint()
            );
        }
    }

    #[test]
    fn rejects_partial_launcher_prefixes() {
        // Complete tokens only: `sase-listenx` or `sase-listen renderx` must
        // not count as the legacy launcher.
        for template in [
            "sase-listenx render {target} -o {audio}",
            "sase-listen renderx {target} -o {audio}",
            "sase-listen2 render {target} -o {audio}",
        ] {
            test_command(template)
                .validate()
                .expect_err("partial launcher must fail");
        }
    }

    #[test]
    fn rejects_malformed_quoting_and_nuls() {
        test_command("sase listen render 'unterminated {target} -o {audio}")
            .validate()
            .expect_err("malformed quoting must fail");
        test_command("sase listen render {target} -o {audio}\0")
            .validate()
            .expect_err("NUL must fail");
    }

    #[test]
    fn rejects_templates_missing_audio_or_source() {
        let missing_audio = test_command("sase listen render {target}");
        let error = missing_audio
            .validate()
            .expect_err("template without {audio} must fail");
        assert!(
            error.message().contains("{audio}"),
            "unexpected message: {}",
            error.message()
        );

        let missing_source = test_command("sase listen render -o {audio}");
        let error = missing_source
            .validate()
            .expect_err("template without {target}/{pdf} must fail");
        assert!(
            error.message().contains("{target}")
                && error.message().contains("{pdf}"),
            "unexpected message: {}",
            error.message()
        );
    }

    #[test]
    fn rejects_unknown_placeholders() {
        let command = test_command(
            "sase listen render {target} -o {audio} --name {targt}",
        );
        let error = command
            .validate()
            .expect_err("unknown placeholder must fail");
        assert!(
            error.message().contains("{targt}"),
            "unexpected message: {}",
            error.message()
        );
    }

    #[test]
    fn rejects_quoted_placeholders() {
        for template in [
            "sase listen render '{target}' -o {audio}",
            "sase listen render \"{target}\" -o {audio}",
            "sase listen render {target} -o '{audio}'",
        ] {
            let error = test_command(template)
                .validate()
                .expect_err("quoted placeholder must fail");
            assert_eq!(
                error.hint(),
                "bob passes placeholder values as single arguments itself; remove the quotes",
                "unexpected hint for {template}"
            );
        }
    }

    /// Fake `sase` executable: asserts the fixed launcher, logs argv
    /// boundaries, strips `listen render`, then writes an ID3 stub to the
    /// `.mp3` argument. Honors `FAKE_LISTEN_EXIT`, `FAKE_LISTEN_NO_AUDIO`,
    /// and `FAKE_LISTEN_TEXT` like the CLI fixtures.
    fn write_fake_sase(bin_dir: &Path) -> PathBuf {
        let script = r#"#!/bin/sh
echo "$@" >> "$FAKE_LISTEN_LOG"
if [ "$1" != "listen" ] || [ "$2" != "render" ]; then
  echo "bad-launcher:$1:$2" >> "$FAKE_LISTEN_LOG"
  exit 99
fi
shift 2
for arg in "$@"; do
  printf 'arg:%s\n' "$arg" >> "$FAKE_LISTEN_LOG"
  case "$arg" in
    *.mp3)
      if [ "${FAKE_LISTEN_EXIT:-0}" = "0" ] && [ -z "$FAKE_LISTEN_NO_AUDIO" ]; then
        if [ -n "$FAKE_LISTEN_TEXT" ]; then
          printf '%s' "$FAKE_LISTEN_TEXT" > "$arg"
        else
          printf 'ID3 fake episode' > "$arg"
        fi
      fi
      ;;
  esac
done
exit "${FAKE_LISTEN_EXIT:-0}"
"#;
        write_executable(bin_dir, "sase", script)
    }

    /// Fake `sase-listen` trap: records any invocation and fails. Its marker
    /// must remain absent on every path (no fallback).
    fn write_sase_listen_trap(bin_dir: &Path, marker: &Path) -> PathBuf {
        let marker_str = marker.display().to_string();
        let script = format!(
            "#!/bin/sh\necho \"trap-hit:$@\" >> \"{marker_str}\"\nexit 99\n"
        );
        write_executable(bin_dir, "sase-listen", &script)
    }

    fn path_with_bin(bin_dir: &Path) -> std::ffi::OsString {
        let old = crate::native::env::var_os("PATH").unwrap_or_default();
        let mut out = bin_dir.as_os_str().to_os_string();
        out.push(":");
        out.push(old);
        out
    }

    #[test]
    fn placeholder_values_stay_single_arguments() {
        // Token expansion keeps spaces, quotes, Unicode, newlines, dollar
        // signs, shell metacharacters, and the text `sase-listen` as data in
        // the same argument; no naive global replacement runs.
        let values = [
            "plain",
            "with spaces",
            "a&b",
            "it's quoted",
            "a$b",
            "a`b`c",
            "line1\nline2",
            "https://example.com/?a=1&b=2",
            "sase-listen render faked",
            "ünïcodé — ♫",
            "",
        ];
        for value in values {
            let command = test_command(
                "sase listen render {target} -e full -o {audio} --title {title}",
            );
            let args = command
                .expand_render_args(&ListenValues {
                    target: "t".to_string(),
                    pdf: PathBuf::from("in.pdf"),
                    audio: PathBuf::from("out.mp3"),
                    title: value.to_string(),
                })
                .expect("valid template must expand");
            assert!(
                args.iter().any(|arg| arg == value),
                "value {value:?} must arrive as one argument: {args:?}"
            );
        }
        // Static quoted data with punctuation stays one token too.
        let command = test_command(
            "sase listen render --note 'a;b | c' {target} -o {audio}",
        );
        let args = command
            .expand_render_args(&test_values(Path::new("/tmp")))
            .expect("quoted static data must expand");
        assert!(
            args.contains(&"a;b | c".to_string()),
            "quoted static data must stay one argument: {args:?}"
        );
    }

    #[test]
    fn run_listen_writes_id3_audio_on_success() {
        let scratch = scratch_dir("ok");
        let bin = scratch.join("bin");
        fs::create_dir_all(&bin).expect("create fake bin");
        let log = scratch.join("listen.log");
        let trap_marker = scratch.join("trap.hit");
        write_fake_sase(&bin);
        write_sase_listen_trap(&bin, &trap_marker);
        let path = path_with_bin(&bin);
        let _guard = crate::native::env::TestEnvGuard::set(&[
            ("PATH", Some(path.as_os_str())),
            ("FAKE_LISTEN_LOG", Some(log.as_os_str())),
            ("FAKE_LISTEN_EXIT", None),
            ("FAKE_LISTEN_NO_AUDIO", None),
            ("FAKE_LISTEN_TEXT", None),
        ]);
        let command =
            test_command("sase listen render {target} -e full -o {audio}");
        let values = test_values(&scratch);
        let audio =
            run_listen(&command, &values).expect("ID3 audio must verify");
        assert_eq!(audio, values.audio);
        assert!(
            fs::read(&values.audio)
                .expect("read written audio")
                .starts_with(b"ID3"),
            "fake sase must have written the ID3 stub"
        );
        let logged = fs::read_to_string(&log).expect("read listen log");
        assert!(
            logged.contains("listen render"),
            "fake saw the fixed launcher: {logged}"
        );
        assert!(!trap_marker.exists(), "legacy trap must not run on success");
    }

    #[test]
    fn legacy_template_migrates_and_preserves_flags() {
        let scratch = scratch_dir("legacy-ok");
        let bin = scratch.join("bin");
        fs::create_dir_all(&bin).expect("create fake bin");
        let log = scratch.join("listen.log");
        let trap_marker = scratch.join("trap.hit");
        write_fake_sase(&bin);
        write_sase_listen_trap(&bin, &trap_marker);
        let path = path_with_bin(&bin);
        let _guard = crate::native::env::TestEnvGuard::set(&[
            ("PATH", Some(path.as_os_str())),
            ("FAKE_LISTEN_LOG", Some(log.as_os_str())),
            ("FAKE_LISTEN_EXIT", None),
            ("FAKE_LISTEN_NO_AUDIO", None),
            ("FAKE_LISTEN_TEXT", None),
        ]);
        let command = test_command(
            "sase-listen render {target} -e full -o {audio} --no-publish",
        );
        let values = test_values(&scratch);
        run_listen(&command, &values).expect("legacy must run via sase");
        let logged = fs::read_to_string(&log).expect("read listen log");
        assert!(
            logged.starts_with("listen render"),
            "legacy ran the fixed launcher: {logged}"
        );
        assert!(
            logged.contains("--no-publish"),
            "custom flags survive migration: {logged}"
        );
        assert!(
            !trap_marker.exists(),
            "legacy must not hit the standalone trap"
        );
        let dry = would_run_line(&command, &values);
        assert!(
            dry.contains("sase listen render"),
            "dry-run uses the normalized launcher: {dry}"
        );
        assert!(
            !dry.contains("sase-listen"),
            "dry-run never shows the legacy launcher: {dry}"
        );
    }

    #[test]
    fn run_listen_accepts_mpeg_frame_sync_audio() {
        let scratch = scratch_dir("frame-sync");
        let audio = scratch.join("episode.mp3");
        fs::write(&audio, [0xFF, 0xFB, 0x90, 0x00])
            .expect("write frame-sync stub");
        assert!(audio_is_mp3(&audio));
    }

    #[test]
    fn run_listen_reports_failed_exits() {
        let scratch = scratch_dir("failed");
        let bin = scratch.join("bin");
        fs::create_dir_all(&bin).expect("create fake bin");
        let log = scratch.join("listen.log");
        let trap_marker = scratch.join("trap.hit");
        write_fake_sase(&bin);
        write_sase_listen_trap(&bin, &trap_marker);
        let path = path_with_bin(&bin);
        let _guard = crate::native::env::TestEnvGuard::set(&[
            ("PATH", Some(path.as_os_str())),
            ("FAKE_LISTEN_LOG", Some(log.as_os_str())),
            ("FAKE_LISTEN_EXIT", Some(std::ffi::OsStr::new("4"))),
            ("FAKE_LISTEN_NO_AUDIO", None),
            ("FAKE_LISTEN_TEXT", None),
        ]);
        let command =
            test_command("sase listen render {target} -e full -o {audio}");
        let values = test_values(&scratch);
        let error =
            run_listen(&command, &values).expect_err("exit 4 must fail");
        assert!(
            matches!(error, ListenError::Failed { .. }),
            "unexpected error: {error}"
        );
        assert!(
            error.message().contains("exit 4"),
            "unexpected message: {}",
            error.message()
        );
        assert!(!values.audio.exists(), "failed runs must not produce audio");
        assert!(
            !trap_marker.exists(),
            "failure must not fall back to the trap"
        );
    }

    #[test]
    fn run_listen_reports_interrupt_exits() {
        let scratch = scratch_dir("interrupted");
        let bin = scratch.join("bin");
        fs::create_dir_all(&bin).expect("create fake bin");
        let log = scratch.join("listen.log");
        write_fake_sase(&bin);
        let path = path_with_bin(&bin);
        let _guard = crate::native::env::TestEnvGuard::set(&[
            ("PATH", Some(path.as_os_str())),
            ("FAKE_LISTEN_LOG", Some(log.as_os_str())),
            ("FAKE_LISTEN_EXIT", Some(std::ffi::OsStr::new("130"))),
            ("FAKE_LISTEN_NO_AUDIO", None),
            ("FAKE_LISTEN_TEXT", None),
        ]);
        let command =
            test_command("sase listen render {target} -e full -o {audio}");
        let error = run_listen(&command, &test_values(&scratch))
            .expect_err("exit 130 must interrupt");
        assert!(
            matches!(error, ListenError::Interrupted { .. }),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn run_listen_rejects_missing_and_non_mp3_audio() {
        let scratch = scratch_dir("no-audio");
        let bin = scratch.join("bin");
        fs::create_dir_all(&bin).expect("create fake bin");
        let log = scratch.join("listen.log");
        write_fake_sase(&bin);
        let path = path_with_bin(&bin);
        let command =
            test_command("sase listen render {target} -e full -o {audio}");
        let values = test_values(&scratch);
        {
            let _guard = crate::native::env::TestEnvGuard::set(&[
                ("PATH", Some(path.as_os_str())),
                ("FAKE_LISTEN_LOG", Some(log.as_os_str())),
                ("FAKE_LISTEN_EXIT", None),
                ("FAKE_LISTEN_NO_AUDIO", Some(std::ffi::OsStr::new("1"))),
                ("FAKE_LISTEN_TEXT", None),
            ]);
            let error = run_listen(&command, &values)
                .expect_err("exit 0 without audio must fail");
            assert!(
                matches!(error, ListenError::NoAudio { .. }),
                "unexpected error: {error}"
            );
            assert!(
                error
                    .message()
                    .contains(&values.audio.display().to_string()),
                "unexpected message: {}",
                error.message()
            );
        }
        {
            let _guard = crate::native::env::TestEnvGuard::set(&[
                ("PATH", Some(path.as_os_str())),
                ("FAKE_LISTEN_LOG", Some(log.as_os_str())),
                ("FAKE_LISTEN_EXIT", None),
                ("FAKE_LISTEN_NO_AUDIO", None),
                (
                    "FAKE_LISTEN_TEXT",
                    Some(std::ffi::OsStr::new("not audio at all")),
                ),
            ]);
            let error = run_listen(&command, &values)
                .expect_err("non-MP3 bytes must fail");
            assert!(
                matches!(error, ListenError::NoAudio { .. }),
                "unexpected error: {error}"
            );
        }
    }

    #[test]
    fn run_listen_has_no_fallback_when_sase_missing() {
        // `sase` absent but `sase-listen` present: no standalone retry.
        let scratch = scratch_dir("no-sase");
        let bin = scratch.join("bin");
        fs::create_dir_all(&bin).expect("create fake bin");
        let trap_marker = scratch.join("trap.hit");
        write_sase_listen_trap(&bin, &trap_marker);
        let path: std::ffi::OsString = {
            let mut out = bin.as_os_str().to_os_string();
            out
        };
        let _guard = crate::native::env::TestEnvGuard::set(&[
            ("PATH", Some(path.as_os_str())),
            ("FAKE_LISTEN_EXIT", None),
            ("FAKE_LISTEN_NO_AUDIO", None),
            ("FAKE_LISTEN_TEXT", None),
        ]);
        let command =
            test_command("sase listen render {target} -e full -o {audio}");
        let values = test_values(&scratch);
        let error =
            run_listen(&command, &values).expect_err("missing sase must fail");
        assert!(
            matches!(error, ListenError::Failed { .. }),
            "unexpected error: {error}"
        );
        assert!(
            error.hint().contains("SASE"),
            "missing launcher must suggest SASE: {}",
            error.hint()
        );
        assert!(
            !trap_marker.exists(),
            "no fallback to sase-listen when sase is absent"
        );
        assert!(!values.audio.exists(), "failed launch writes no audio");
    }

    #[test]
    fn run_listen_validates_before_running() {
        let scratch = scratch_dir("invalid");
        // Missing {audio} still fails before any subprocess runs.
        let command = test_command("sase listen render {target}");
        let error = run_listen(&command, &test_values(&scratch))
            .expect_err("invalid template must fail before running");
        assert!(
            matches!(error, ListenError::Invalid { .. }),
            "unexpected error: {error}"
        );
    }

    fn with_listen_env(
        value: Option<&str>,
        config: Option<&str>,
        check: impl FnOnce(),
    ) {
        // Thread-local overrides: never touch process environment, so no
        // lock is needed and parallel tests cannot observe these values.
        let _guard = crate::native::env::TestEnvGuard::set(&[
            (ENV_LISTEN_COMMAND, value.map(std::ffi::OsStr::new)),
            ("BOB_CONFIG_FILE", config.map(std::ffi::OsStr::new)),
        ]);
        check();
    }

    #[test]
    fn env_overrides_config() {
        let scratch = scratch_dir("precedence");
        let config_path = scratch.join("config.yml");
        fs::write(
            &config_path,
            "highlights:\n  listen_command: 'sase listen render {target} -o {audio} --from-config'\n",
        )
        .expect("write test config");
        let config = config_path.to_string_lossy().into_owned();

        with_listen_env(
            Some("sase listen render {target} -o {audio} --from-env"),
            Some(&config),
            || {
                assert_eq!(
                    ListenCommand::resolve()
                        .expect("resolve succeeds")
                        .as_ref()
                        .map(ListenCommand::display),
                    Some("sase listen render {target} -o {audio} --from-env"),
                    "environment must win over the config file"
                );
            },
        );

        with_listen_env(None, Some(&config), || {
            assert_eq!(
                ListenCommand::resolve()
                    .expect("resolve succeeds")
                    .as_ref()
                    .map(ListenCommand::display),
                Some("sase listen render {target} -o {audio} --from-config"),
                "config applies without the environment override"
            );
        });
    }

    #[test]
    fn invalid_wrappers_fail_before_any_run() {
        // Wrappers/chains fail validation, so no subprocess ever runs.
        for template in [
            "sh -c 'sase listen render {target} -o {audio}'",
            "sase listen render {target} -o {audio}; echo hi",
            "my-narrator {target} -o {audio}",
        ] {
            let error = test_command(template)
                .validate()
                .expect_err("wrapper must fail");
            assert!(
                error.hint().contains("sase listen render {target}"),
                "unexpected hint for {template:?}: {}",
                error.hint()
            );
            let values = test_values(Path::new("/tmp"));
            let dry = would_run_line(&test_command(template), &values);
            assert!(
                !dry.contains("listen: run"),
                "invalid dry-run must not look like a run: {dry}"
            );
        }
    }

    #[test]
    fn blank_env_and_missing_config_mean_unset() {
        let scratch = scratch_dir("unset");
        let missing = scratch.join("definitely-missing-config.yml");
        let missing = missing.to_string_lossy().into_owned();

        with_listen_env(Some("   "), Some(&missing), || {
            assert_eq!(
                ListenCommand::resolve().expect("resolve succeeds"),
                None,
                "blank environment must mean unset"
            );
        });
        with_listen_env(None, Some(&missing), || {
            assert_eq!(
                ListenCommand::resolve().expect("resolve succeeds"),
                None,
                "missing config must mean unset"
            );
        });
    }

    #[test]
    fn require_reports_the_config_path_and_snippet() {
        let scratch = scratch_dir("require");
        let missing = scratch.join("definitely-missing-config.yml");
        let missing = missing.to_string_lossy().into_owned();
        with_listen_env(None, Some(&missing), || {
            let error = ListenCommand::require()
                .expect_err("unconfigured --listen must fail");
            assert!(
                matches!(error, ListenError::NotConfigured { .. }),
                "unexpected error: {error}"
            );
            assert!(
                error.message().contains("highlights.listen_command")
                    && error.message().contains(ENV_LISTEN_COMMAND),
                "unexpected message: {}",
                error.message()
            );
            assert!(
                error.hint().contains(
                    "listen_command: sase listen render {target} -e full -o {audio}"
                ),
                "unexpected hint: {}",
                error.hint()
            );
        });
    }

    #[test]
    fn quiet_runner_preserves_typed_failures() {
        // The shared Quiet path (background ingest) keeps JSON stdout clean
        // by sending everything to stderr, with the same typed errors.
        let scratch = scratch_dir("quiet");
        let bin = scratch.join("bin");
        fs::create_dir_all(&bin).expect("create fake bin");
        let log = scratch.join("listen.log");
        let trap_marker = scratch.join("trap.hit");
        write_fake_sase(&bin);
        write_sase_listen_trap(&bin, &trap_marker);
        let path = path_with_bin(&bin);
        let command =
            test_command("sase listen render {target} -e full -o {audio}");
        let values = test_values(&scratch);
        {
            let _guard = crate::native::env::TestEnvGuard::set(&[
                ("PATH", Some(path.as_os_str())),
                ("FAKE_LISTEN_LOG", Some(log.as_os_str())),
                ("FAKE_LISTEN_EXIT", None),
                ("FAKE_LISTEN_NO_AUDIO", None),
                ("FAKE_LISTEN_TEXT", None),
            ]);
            let audio =
                run_listen_with_output(&command, &values, ListenOutput::Quiet)
                    .expect("quiet success must verify");
            assert_eq!(audio, values.audio);
        }
        {
            let _guard = crate::native::env::TestEnvGuard::set(&[
                ("PATH", Some(path.as_os_str())),
                ("FAKE_LISTEN_LOG", Some(log.as_os_str())),
                ("FAKE_LISTEN_EXIT", Some(std::ffi::OsStr::new("4"))),
                ("FAKE_LISTEN_NO_AUDIO", None),
                ("FAKE_LISTEN_TEXT", None),
            ]);
            let error = run_listen_with_output(
                &command,
                &test_values(&scratch),
                ListenOutput::Quiet,
            )
            .expect_err("quiet exit 4 must fail");
            assert!(
                matches!(error, ListenError::Failed { .. }),
                "unexpected error: {error}"
            );
        }
        {
            // Remove the audio written by the success block: NoAudio must be
            // proven by absence, not by leftover bytes.
            fs::remove_file(&values.audio).ok();
            let _guard = crate::native::env::TestEnvGuard::set(&[
                ("PATH", Some(path.as_os_str())),
                ("FAKE_LISTEN_LOG", Some(log.as_os_str())),
                ("FAKE_LISTEN_EXIT", None),
                ("FAKE_LISTEN_NO_AUDIO", Some(std::ffi::OsStr::new("1"))),
                ("FAKE_LISTEN_TEXT", None),
            ]);
            let error =
                run_listen_with_output(&command, &values, ListenOutput::Quiet)
                    .expect_err("quiet no-audio must fail");
            assert!(
                matches!(error, ListenError::NoAudio { .. }),
                "unexpected error: {error}"
            );
        }
        assert!(
            !trap_marker.exists(),
            "quiet path never falls back to the trap"
        );
    }
}

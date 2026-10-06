//! Configurable listen command contract and runner (`--listen`).
//!
//! The command comes from `BOB_HIGHLIGHTS_LISTEN_COMMAND`, and otherwise from
//! `highlights.listen_command`. It must write MP3 audio to `{audio}`; bob
//! shell-quotes the `{target}`, `{pdf}`, `{audio}`, and `{title}` values
//! itself, streams the command's output unchanged, and verifies the MP3.
use super::*;

pub(super) const ENV_LISTEN_COMMAND: &str = "BOB_HIGHLIGHTS_LISTEN_COMMAND";

/// Hint printed when `--listen` is requested without a configured command.
const NOT_CONFIGURED_HINT: &str = "add `listen_command: sase-listen render {target} -e full -o {audio}` under `highlights:`";
/// Hint printed when the listen command itself fails: nothing was written.
const LISTEN_FAILED_HINT: &str = "nothing was written to the vault; rerun the same command once the listen error above is fixed";
/// Hint printed when a placeholder is wrapped in quotes.
const QUOTED_PLACEHOLDER_HINT: &str =
    "bob shell-quotes placeholder values itself; remove the quotes";

const KNOWN_PLACEHOLDERS: &[&str] = &["target", "pdf", "audio", "title"];

/// Values substituted into a [`ListenCommand`] template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ListenValues {
    pub(super) target: String,
    pub(super) pdf: PathBuf,
    pub(super) audio: PathBuf,
    pub(super) title: String,
}

/// A validated-or-not shell command template that narrates a target and
/// writes MP3 audio to `{audio}`.
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
        CommandError::new(format!("{}\nhint: {}", self.message(), self.hint()))
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
        if let Some(command) = env::var_os(ENV_LISTEN_COMMAND) {
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

    /// Check the template contract: `{audio}` plus `{target}` or `{pdf}`
    /// must be present, every placeholder must be known, and none may be
    /// wrapped in quotes (bob quotes values itself).
    pub(super) fn validate(&self) -> std::result::Result<(), ListenError> {
        if let Some(name) = quoted_placeholder(&self.command) {
            return Err(invalid(
                format!(
                    "highlights.listen_command quotes placeholder {{{name}}}; \
                     bob shell-quotes placeholder values itself"
                ),
                QUOTED_PLACEHOLDER_HINT,
            ));
        }
        let placeholders = find_placeholders(&self.command);
        for name in &placeholders {
            if !KNOWN_PLACEHOLDERS.contains(&name.as_str()) {
                return Err(invalid(
                    format!(
                        "unknown placeholder {{{name}}} in \
                         highlights.listen_command (expected {{target}}, \
                         {{pdf}}, {{audio}}, or {{title}})"
                    ),
                    "use only {target}, {pdf}, {audio}, and {title}; \
                     ${VAR} and {a,b} pass through to the shell",
                ));
            }
        }
        if !placeholders.iter().any(|name| name == "audio") {
            return Err(invalid(
                "highlights.listen_command must write MP3 audio to {audio}"
                    .to_string(),
                "add {audio} to the command, for example: \
                 sase-listen render {target} -e full -o {audio}",
            ));
        }
        if !placeholders
            .iter()
            .any(|name| name == "target" || name == "pdf")
        {
            return Err(invalid(
                "highlights.listen_command must contain {target} or {pdf}"
                    .to_string(),
                "add the narration source, for example: \
                 sase-listen render {target} -e full -o {audio}",
            ));
        }
        Ok(())
    }

    /// Substitute the values into the template, POSIX-shell-quoting each
    /// one. `${VAR}` and `{a,b}` pass through to the shell unchanged.
    pub(super) fn expand(&self, values: &ListenValues) -> String {
        let mut expanded = String::with_capacity(self.command.len());
        let mut cursor = 0;
        while cursor < self.command.len() {
            let is_placeholder_open = self.command.as_bytes()[cursor] == b'{'
                && (cursor == 0 || self.command.as_bytes()[cursor - 1] != b'$');
            if is_placeholder_open
                && let Some((name, end)) = placeholder_at(&self.command, cursor)
                && let Some(quoted) = substitute_placeholder(name, values)
            {
                expanded.push_str(&quoted);
                cursor = end;
                continue;
            }
            let character =
                self.command[cursor..].chars().next().unwrap_or('\0');
            if character == '\0' {
                break;
            }
            expanded.push(character);
            cursor += character.len_utf8();
        }
        expanded
    }
}

fn substitute_placeholder(name: &str, values: &ListenValues) -> Option<String> {
    match name {
        "target" => Some(shell_quote(&values.target)),
        "pdf" => Some(shell_quote(&values.pdf.to_string_lossy())),
        "audio" => Some(shell_quote(&values.audio.to_string_lossy())),
        "title" => Some(shell_quote(&values.title)),
        _ => None,
    }
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

/// Collect every `{name}` placeholder in order. A `{` preceded by `$` is
/// shell syntax (`${HOME}`), not a placeholder.
fn find_placeholders(template: &str) -> Vec<String> {
    let mut placeholders = Vec::new();
    let mut cursor = 0;
    while cursor < template.len() {
        let is_open = template.as_bytes()[cursor] == b'{'
            && (cursor == 0 || template.as_bytes()[cursor - 1] != b'$');
        if is_open && let Some((name, end)) = placeholder_at(template, cursor) {
            placeholders.push(name.to_string());
            cursor = end;
        } else {
            let character = template[cursor..].chars().next().unwrap_or('\0');
            if character == '\0' {
                break;
            }
            cursor += character.len_utf8();
        }
    }
    placeholders
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

/// Run the listen command with the values substituted in, streaming its
/// output unchanged (stdin, stdout, and stderr are inherited). Prints
/// `listen: run <expanded command>` first. On exit 0 the MP3 at `{audio}`
/// is verified and its path returned.
pub(super) fn run_listen(
    command: &ListenCommand,
    values: &ListenValues,
) -> std::result::Result<PathBuf, ListenError> {
    command.validate()?;
    let expanded = command.expand(values);
    println!("listen: run {expanded}");
    {
        use std::io::Write as _;
        let _ = std::io::stdout().flush();
    }

    // Like `system(3)`: ignore SIGINT/SIGQUIT while the child runs so
    // Ctrl-C reaches the listen command's graceful stop, and reset both to
    // the default dispositions in the child.
    let previous_signals = parent_ignore_signals();
    let mut child = process::Command::new("sh");
    child
        .arg("-c")
        .arg(&expanded)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // SAFETY: `reset_child_signals` only calls the async-signal-safe
        // `signal` between `fork` and `exec`.
        unsafe {
            child.pre_exec(reset_child_signals);
        }
    }
    let status = child.status();
    parent_restore_signals(previous_signals);
    let status = status.map_err(|error| ListenError::Failed {
        message: format!("run listen command {expanded}: {error}"),
        hint: LISTEN_FAILED_HINT.to_string(),
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
            hint: "make sure the command writes MP3 audio to {audio}"
                .to_string(),
        });
    }
    if status_interrupted(&status) {
        return Err(ListenError::Interrupted {
            message: "listen command interrupted".to_string(),
            hint: "rerun the same command to try again".to_string(),
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

/// Non-fatal `doctor` row for the listen command, printed after the pandoc
/// and web-clip rows. Problems warn; they never fail the vault doctor.
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
            "highlights listen command is invalid: {}",
            error.message()
        ));
        return;
    }
    let hook = PreScanHook {
        command: OsString::from(command.display()),
    };
    let Some(program) = pre_scan_program(&hook) else {
        println!(
            "listen_command: warn ({}; executable not found)",
            command.display()
        );
        warnings.push(format!(
            "highlights listen command has no executable word: {}",
            command.display()
        ));
        return;
    };
    match shell_command_available(&program) {
        Ok(true) => {
            println!(
                "listen_command: ok ({}; executable: {})",
                command.display(),
                program.to_string_lossy()
            );
        }
        Ok(false) => {
            println!(
                "listen_command: warn ({}; executable not found: {})",
                command.display(),
                program.to_string_lossy()
            );
            warnings.push(format!(
                "highlights listen command executable not found: {}",
                program.to_string_lossy()
            ));
        }
        Err(error) => {
            println!(
                "listen_command: warn ({}; executable check failed: {error})",
                command.display()
            );
            warnings.push(format!(
                "highlights listen command executable check failed: {error}"
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
    use std::sync::Mutex;

    /// Serializes the tests that mutate process environment.
    static ENV_LOCK: Mutex<()> = Mutex::new(());
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
    fn accepts_complete_and_shell_syntax_templates() {
        for template in [
            "sase-listen render {target} -e full -o {audio}",
            "sase-listen render {pdf} -e full -o {audio} --title {title}",
            "${HOME}/bin/render {target} -o {audio}",
            "render {target} --fmt {a,b} -o {audio}",
            "render {target} -o {audio} ${HOME} {a,b} {} {Target} {AUDIO}",
        ] {
            test_command(template)
                .validate()
                .expect("template must validate");
        }
    }

    #[test]
    fn rejects_templates_missing_audio_or_source() {
        let missing_audio = test_command("sase-listen render {target}");
        let error = missing_audio
            .validate()
            .expect_err("template without {audio} must fail");
        assert!(
            error.message().contains("{audio}"),
            "unexpected message: {}",
            error.message()
        );

        let missing_source = test_command("sase-listen render -o {audio}");
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
            "sase-listen render {target} -o {audio} --name {targt}",
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
            "sase-listen render '{target}' -o {audio}",
            "sase-listen render \"{target}\" -o {audio}",
            "sase-listen render {target} -o '{audio}'",
        ] {
            let error = test_command(template)
                .validate()
                .expect_err("quoted placeholder must fail");
            assert_eq!(
                error.hint(),
                "bob shell-quotes placeholder values itself; remove the quotes",
                "unexpected hint for {template}"
            );
        }
    }

    #[test]
    fn quoting_round_trips_through_shell() {
        let values = [
            "plain",
            "with spaces",
            "a&b",
            "it's quoted",
            "a$b",
            "a`b`c",
            "line1\nline2",
            "https://example.com/?a=1&b=2",
            "",
        ];
        for value in values {
            let command = test_command("printf '%s\\n' {title}");
            let expanded = command.expand(&ListenValues {
                target: String::new(),
                pdf: PathBuf::from("in.pdf"),
                audio: PathBuf::from("out.mp3"),
                title: value.to_string(),
            });
            let output = process::Command::new("sh")
                .arg("-c")
                .arg(&expanded)
                .output()
                .expect("run quoted round-trip");
            assert!(
                output.status.success(),
                "expanded command failed: {expanded}"
            );
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                format!("{value}\n"),
                "round-trip mismatch for {value:?} ({expanded})"
            );
        }
    }

    #[test]
    fn run_listen_writes_id3_audio_on_success() {
        let scratch = scratch_dir("ok");
        let script = write_executable(
            &scratch,
            "fake-listen-ok.sh",
            "#!/bin/sh\nprintf 'ID3\\x04\\x00fake-mp3-payload' > \"$2\"\n",
        );
        let command =
            test_command(&format!("{} {{target}} {{audio}}", script.display()));
        let values = test_values(&scratch);
        let audio =
            run_listen(&command, &values).expect("ID3 audio must verify");
        assert_eq!(audio, values.audio);
        assert!(
            fs::read(&values.audio)
                .expect("read written audio")
                .starts_with(b"ID3"),
            "fake script must have written the ID3 stub"
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
        let script = write_executable(
            &scratch,
            "fake-listen-fail.sh",
            "#!/bin/sh\nexit 4\n",
        );
        let command =
            test_command(&format!("{} {{target}} {{audio}}", script.display()));
        let error = run_listen(&command, &test_values(&scratch))
            .expect_err("exit 4 must fail");
        assert!(
            matches!(error, ListenError::Failed { .. }),
            "unexpected error: {error}"
        );
        assert!(
            error.message().contains("exit 4"),
            "unexpected message: {}",
            error.message()
        );
        assert!(
            !test_values(&scratch).audio.exists(),
            "failed runs must not produce audio"
        );
    }

    #[test]
    fn run_listen_reports_interrupt_exits() {
        let scratch = scratch_dir("interrupted");
        let script = write_executable(
            &scratch,
            "fake-listen-interrupt.sh",
            "#!/bin/sh\nexit 130\n",
        );
        let command =
            test_command(&format!("{} {{target}} {{audio}}", script.display()));
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
        let silent = write_executable(
            &scratch,
            "fake-listen-silent.sh",
            "#!/bin/sh\nexit 0\n",
        );
        let command =
            test_command(&format!("{} {{target}} {{audio}}", silent.display()));
        let values = test_values(&scratch);
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

        let noisy = write_executable(
            &scratch,
            "fake-listen-noisy.sh",
            "#!/bin/sh\nprintf 'not audio at all' > \"$2\"\nexit 0\n",
        );
        let command =
            test_command(&format!("{} {{target}} {{audio}}", noisy.display()));
        let error =
            run_listen(&command, &values).expect_err("non-MP3 bytes must fail");
        assert!(
            matches!(error, ListenError::NoAudio { .. }),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn run_listen_validates_before_running() {
        let scratch = scratch_dir("invalid");
        let command = test_command("sase-listen render {target}");
        let error = run_listen(&command, &test_values(&scratch))
            .expect_err("invalid template must fail before running");
        assert!(
            matches!(error, ListenError::Invalid { .. }),
            "unexpected error: {error}"
        );
    }

    fn with_env(
        value: Option<&str>,
        config: Option<&str>,
        check: impl FnOnce(),
    ) {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let old_listen = env::var_os(ENV_LISTEN_COMMAND);
        let old_config = env::var_os("BOB_CONFIG_FILE");
        // SAFETY: the mutex serializes every environment mutation in this
        // test binary, and both values are restored before returning.
        unsafe {
            match value {
                Some(value) => env::set_var(ENV_LISTEN_COMMAND, value),
                None => env::remove_var(ENV_LISTEN_COMMAND),
            }
            match config {
                Some(config) => env::set_var("BOB_CONFIG_FILE", config),
                None => env::remove_var("BOB_CONFIG_FILE"),
            }
        }
        check();
        // SAFETY: same serialized context as above.
        unsafe {
            match old_listen {
                Some(value) => env::set_var(ENV_LISTEN_COMMAND, value),
                None => env::remove_var(ENV_LISTEN_COMMAND),
            }
            match old_config {
                Some(value) => env::set_var("BOB_CONFIG_FILE", value),
                None => env::remove_var("BOB_CONFIG_FILE"),
            }
        }
    }

    #[test]
    fn env_overrides_config() {
        let scratch = scratch_dir("precedence");
        let config_path = scratch.join("config.yml");
        fs::write(
            &config_path,
            "highlights:\n  listen_command: 'from-config {target} -o {audio}'\n",
        )
        .expect("write test config");
        let config = config_path.to_string_lossy().into_owned();

        with_env(Some("from-env {target} -o {audio}"), Some(&config), || {
            assert_eq!(
                ListenCommand::resolve()
                    .expect("resolve succeeds")
                    .as_ref()
                    .map(ListenCommand::display),
                Some("from-env {target} -o {audio}"),
                "environment must win over the config file"
            );
        });

        with_env(None, Some(&config), || {
            assert_eq!(
                ListenCommand::resolve()
                    .expect("resolve succeeds")
                    .as_ref()
                    .map(ListenCommand::display),
                Some("from-config {target} -o {audio}"),
                "config applies without the environment override"
            );
        });
    }

    #[test]
    fn blank_env_and_missing_config_mean_unset() {
        let scratch = scratch_dir("unset");
        let missing = scratch.join("definitely-missing-config.yml");
        let missing = missing.to_string_lossy().into_owned();

        with_env(Some("   "), Some(&missing), || {
            assert_eq!(
                ListenCommand::resolve().expect("resolve succeeds"),
                None,
                "blank environment must mean unset"
            );
        });
        with_env(None, Some(&missing), || {
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
        with_env(None, Some(&missing), || {
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
                    "listen_command: sase-listen render {target} -e full -o {audio}"
                ),
                "unexpected hint: {}",
                error.hint()
            );
        });
    }
}

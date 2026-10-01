//! `bob highlights clip` adapter client: spawn the pinned web-clip adapter.
//!
//! The client writes one compact JSON request to the adapter's stdin and
//! reads one JSON response from stdout. `BOB_WEB_CLIP_ADAPTER` replaces the
//! `uv run --quiet --script …` invocation; it is the test hook, the same
//! idea as `BOB_GKEEP_ADAPTER`. Every request carries `"protocol": 1`.
//! `BOB_WEB_CLIP_TIMEOUT_SECS` sets the overall adapter timeout.

use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use super::CommandError;

type AdapterResult<T> = std::result::Result<T, AdapterFailure>;

pub(super) const ADAPTER_PROTOCOL_VERSION: u32 = 1;
pub(super) const ENV_ADAPTER_OVERRIDE: &str = "BOB_WEB_CLIP_ADAPTER";
pub(super) const ENV_TIMEOUT_SECS: &str = "BOB_WEB_CLIP_TIMEOUT_SECS";
pub(super) const DEFAULT_TIMEOUT_SECS: u64 = 300;
/// Timeout for the `doctor` health ping.
pub(super) const PING_TIMEOUT_SECS: u64 = 120;

/// A typed adapter failure: the message goes after `error:`, the hint
/// after `hint:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AdapterFailure {
    pub(super) message: String,
    pub(super) hint: Option<String>,
}

impl AdapterFailure {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hint: None,
        }
    }

    fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub(super) fn into_command_error(self) -> CommandError {
        let mut message = self.message;
        if let Some(hint) = self.hint {
            message.push_str(&format!("\nhint: {hint}"));
        }
        CommandError::new(message)
    }
}

impl From<AdapterFailure> for CommandError {
    fn from(failure: AdapterFailure) -> Self {
        failure.into_command_error()
    }
}

/// The resolved adapter invocation: a program plus its argv.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ClipAdapterClient {
    program: PathBuf,
    args: Vec<OsString>,
    timeout: Duration,
}

impl ClipAdapterClient {
    pub(super) fn new(
        program: PathBuf,
        args: Vec<OsString>,
        timeout: Duration,
    ) -> Self {
        Self {
            program,
            args,
            timeout,
        }
    }

    /// Resolve the adapter command: the `BOB_WEB_CLIP_ADAPTER` override, or
    /// `uv run --quiet --script <materialized adapter>`.
    pub(super) fn resolve() -> AdapterResult<Self> {
        Self::resolve_with(adapter_override(), timeout_secs(), &find_on_path)
    }

    /// Resolve a client with a fixed timeout (used by the doctor ping).
    pub(super) fn resolve_with_timeout(secs: u64) -> AdapterResult<Self> {
        Self::resolve_with(adapter_override(), secs, &find_on_path)
    }

    fn resolve_with(
        adapter_override: Option<PathBuf>,
        timeout_secs: u64,
        find_on_path: &dyn Fn(&str) -> Option<PathBuf>,
    ) -> AdapterResult<Self> {
        let timeout = Duration::from_secs(timeout_secs.max(1));
        if let Some(path) = adapter_override {
            return Ok(Self::new(path, Vec::new(), timeout));
        }
        let uv = find_on_path("uv").ok_or_else(|| {
            AdapterFailure::new("uv was not found on PATH").with_hint(
                "install uv (https://docs.astral.sh/uv/) — bob highlights \
                 clip runs its pinned web capture adapter with it",
            )
        })?;
        let script = materialized_adapter_script()?;
        if !script.is_file() {
            return Err(AdapterFailure::new(format!(
                "web clip adapter is not installed: {}",
                script.display()
            ))
            .with_hint(
                "this bob build predates the web clip adapter; rebuild after \
                 the adapter-capture phase lands, or set \
                 BOB_WEB_CLIP_ADAPTER=/path/to/adapter",
            ));
        }
        Ok(Self::new(
            uv,
            vec![
                OsString::from("run"),
                OsString::from("--quiet"),
                OsString::from("--script"),
                script.into_os_string(),
            ],
            timeout,
        ))
    }

    /// Check adapter health: no capture, no writes.
    pub(super) fn ping(&self) -> AdapterResult<PingResponse> {
        let (value, stderr_text) = self.run_request(&PingRequest::new())?;
        into_success(value, &stderr_text, "ping")
    }

    /// Run one capture and return the typed success response.
    pub(super) fn capture(
        &self,
        request: &CaptureRequest,
    ) -> AdapterResult<CaptureSuccess> {
        let (value, stderr_text) = self.run_request(request)?;
        into_success(value, &stderr_text, "capture")
    }

    /// Spawn the adapter, write one request, and read one response.
    ///
    /// Stdin, stdout, and stderr each drain on their own thread while the
    /// main thread polls `try_wait` against the configured deadline. On
    /// timeout the whole adapter process group is killed, so an orphaned
    /// `uv` child cannot hold the pipes open and block the reader joins.
    /// After the leader exits, stragglers are killed before joining the
    /// readers, so nothing holding the pipes can hang the joins.
    fn run_request(
        &self,
        request: &impl Serialize,
    ) -> AdapterResult<(serde_json::Value, String)> {
        let payload =
            serde_json::to_string(request).expect("adapter request serializes");
        let mut child = spawn_adapter(&self.program, &self.args)?;
        let deadline = Instant::now() + self.timeout;
        let stdin_pipe = child.stdin.take();
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();
        let stdin_writer = thread::spawn(move || {
            let mut stdin = stdin_pipe?;
            {
                use std::io::Write;
                if let Err(error) = stdin.write_all(payload.as_bytes()) {
                    return Some(error);
                }
            }
            None
        });
        let stdout_reader = thread::spawn(move || drain_pipe(stdout_pipe));
        let stderr_reader = thread::spawn(move || drain_pipe(stderr_pipe));
        let status = loop {
            match child.try_wait().map_err(|error| {
                AdapterFailure::new(format!(
                    "wait for the web clip adapter: {error}"
                ))
            })? {
                Some(status) => break status,
                None => {
                    if Instant::now() >= deadline {
                        kill_adapter_tree(&mut child);
                        let _ = stdin_writer.join();
                        let _ = stdout_reader.join();
                        let _ = stderr_reader.join();
                        return Err(AdapterFailure::new(format!(
                            "the web clip adapter timed out after {}s",
                            self.timeout.as_secs()
                        ))
                        .with_hint(
                            "check your network connection, or raise \
                             BOB_WEB_CLIP_TIMEOUT_SECS",
                        ));
                    }
                    thread::sleep(Duration::from_millis(10));
                }
            }
        };
        kill_stragglers(child.id());
        let stdin_error = stdin_writer.join().unwrap_or(None);
        let stdout_text = stdout_reader.join().unwrap_or_default();
        let stderr_text = stderr_reader.join().unwrap_or_default();
        match status.code() {
            Some(0) => {}
            Some(code) => {
                if let Some(error) = stdin_error
                    && stdout_text.trim().is_empty()
                    && stderr_text.trim().is_empty()
                {
                    return Err(adapter_crash(
                        format!("write the adapter request: {error}"),
                        stderr_text,
                    ));
                }
                return Err(adapter_crash(
                    format!("the web clip adapter crashed (exit {code})"),
                    stderr_text,
                ));
            }
            None => {
                if let Some(error) = stdin_error
                    && stdout_text.trim().is_empty()
                    && stderr_text.trim().is_empty()
                {
                    return Err(adapter_crash(
                        format!("write the adapter request: {error}"),
                        stderr_text,
                    ));
                }
                return Err(adapter_crash(
                    "the web clip adapter was terminated by a signal"
                        .to_string(),
                    stderr_text,
                ));
            }
        }
        let value: serde_json::Value = serde_json::from_str(&stdout_text)
            .map_err(|error| {
                adapter_crash(
                    format!(
                        "the web clip adapter returned invalid JSON: {error}"
                    ),
                    stderr_text.clone(),
                )
            })?;
        Ok((value, stderr_text))
    }
}

#[derive(Debug, Clone, Serialize)]
struct PingRequest {
    protocol: u32,
    op: &'static str,
}

impl PingRequest {
    fn new() -> Self {
        Self {
            protocol: ADAPTER_PROTOCOL_VERSION,
            op: "ping",
        }
    }
}

/// The adapter's `ping` response: versions, browser, and headed fallback.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct PingResponse {
    #[allow(dead_code)]
    pub(super) protocol: u32,
    #[allow(dead_code)]
    #[serde(default)]
    pub(super) ok: Option<bool>,
    #[allow(dead_code)]
    #[serde(default)]
    pub(super) op: Option<String>,
    #[serde(default)]
    pub(super) playwright: Option<String>,
    #[serde(default)]
    pub(super) defuddle: Option<String>,
    #[serde(default)]
    pub(super) browser: Option<BrowserInfo>,
    #[serde(default)]
    pub(super) headed: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct BrowserInfo {
    #[serde(default)]
    pub(super) kind: Option<String>,
    #[serde(default)]
    pub(super) path: Option<String>,
    #[serde(default)]
    pub(super) version: Option<String>,
}

/// Optional metadata overrides forwarded to the adapter; overrides win.
#[derive(Debug, Clone, Default, Serialize)]
pub(super) struct MetadataOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) published: Option<String>,
}

/// One `capture` request.
#[derive(Debug, Clone, Serialize)]
pub(super) struct CaptureRequest {
    pub(super) protocol: u32,
    pub(super) op: &'static str,
    pub(super) url: String,
    pub(super) html_path: Option<String>,
    pub(super) workdir: String,
    pub(super) out_pdf: String,
    pub(super) dry_run: bool,
    pub(super) captured: String,
    pub(super) overrides: MetadataOverrides,
}

impl CaptureRequest {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        url: String,
        html_path: Option<String>,
        workdir: String,
        out_pdf: String,
        dry_run: bool,
        captured: String,
        overrides: MetadataOverrides,
    ) -> Self {
        Self {
            protocol: ADAPTER_PROTOCOL_VERSION,
            op: "capture",
            url,
            html_path,
            workdir,
            out_pdf,
            dry_run,
            captured,
            overrides,
        }
    }
}

/// A successful `capture` response.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub(super) struct CaptureSuccess {
    #[serde(default)]
    pub(super) kind: Option<String>,
    #[serde(default)]
    pub(super) final_url: Option<String>,
    #[serde(default)]
    pub(super) title: Option<String>,
    #[serde(default)]
    pub(super) author: Option<String>,
    #[serde(default)]
    pub(super) published: Option<String>,
    #[serde(default)]
    pub(super) site: Option<String>,
    #[serde(default)]
    pub(super) description: Option<String>,
    #[serde(default)]
    pub(super) metadata_sources: BTreeMap<String, String>,
    #[serde(default)]
    pub(super) word_count: u64,
    #[serde(default)]
    pub(super) capture: CaptureReport,
    #[serde(default)]
    pub(super) fidelity: FidelityReport,
    #[serde(default)]
    pub(super) images: ImageReport,
    #[serde(default)]
    pub(super) pdf_bytes: Option<u64>,
    #[serde(default)]
    pub(super) warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(super) struct CaptureReport {
    #[serde(default)]
    pub(super) browser: Option<String>,
    #[serde(default)]
    pub(super) browser_version: Option<String>,
    #[serde(default)]
    pub(super) mode: Option<String>,
    #[serde(default)]
    pub(super) retried_after_challenge: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[allow(dead_code)]
pub(super) struct FidelityReport {
    #[serde(default)]
    pub(super) status: Option<String>,
    #[serde(default)]
    pub(super) page_large_media: u64,
    #[serde(default)]
    pub(super) kept_large_media: u64,
    #[serde(default)]
    pub(super) page_code_blocks: u64,
    #[serde(default)]
    pub(super) kept_code_blocks: u64,
    #[serde(default)]
    pub(super) page_words: u64,
    #[serde(default)]
    pub(super) kept_words: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[allow(dead_code)]
pub(super) struct ImageReport {
    #[serde(default)]
    pub(super) total: u64,
    #[serde(default)]
    pub(super) kept: u64,
    #[serde(default)]
    pub(super) skipped_small: u64,
    #[serde(default)]
    pub(super) failed: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct AdapterErrorBody {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    hint: Option<String>,
}

/// Check the protocol version, then split an `ok: false` response into a
/// typed failure or parse a success response.
fn into_success<T: for<'de> Deserialize<'de>>(
    value: serde_json::Value,
    stderr_text: &str,
    op: &str,
) -> AdapterResult<T> {
    let protocol = value.get("protocol").and_then(serde_json::Value::as_u64);
    if protocol != Some(u64::from(ADAPTER_PROTOCOL_VERSION)) {
        return Err(AdapterFailure::new(format!(
            "the web clip adapter spoke protocol {protocol:?}, expected {}",
            ADAPTER_PROTOCOL_VERSION
        )));
    }
    if value.get("ok") != Some(&serde_json::Value::Bool(true)) {
        let body: AdapterErrorBody = serde_json::from_value(
            value
                .get("error")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        )
        .unwrap_or(AdapterErrorBody {
            kind: None,
            message: None,
            hint: None,
        });
        let kind = body.kind.unwrap_or_else(|| "adapter".to_string());
        let mut failure =
            AdapterFailure::new(body.message.unwrap_or_else(|| {
                format!("the web clip adapter failed {op} without detail")
            }));
        if !failure.message.starts_with(&kind) {
            failure.message = format!("{kind}: {}", failure.message);
        }
        if let Some(hint) = body.hint {
            failure = failure.with_hint(hint);
        } else {
            failure = failure.with_stderr_tail(stderr_text);
        }
        return Err(failure);
    }
    serde_json::from_value(value).map_err(|error| {
        AdapterFailure::new(format!("parse the {op} response: {error}"))
    })
}

impl AdapterFailure {
    fn with_stderr_tail(mut self, stderr_text: &str) -> Self {
        let mut tail: Vec<&str> = stderr_text.lines().rev().take(5).collect();
        tail.reverse();
        if !tail.is_empty() {
            self.message.push('\n');
            self.message.push_str(&tail.join("\n"));
        }
        self
    }
}

fn adapter_override() -> Option<PathBuf> {
    env::var_os(ENV_ADAPTER_OVERRIDE)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn timeout_secs() -> u64 {
    env::var_os(ENV_TIMEOUT_SECS)
        .and_then(|value| value.into_string().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
}

/// Materialize the embedded adapter scripts and return the cache dir.
fn materialized_adapter_script() -> AdapterResult<PathBuf> {
    let dir = crate::runner::materialize_scripts().map_err(|error| {
        AdapterFailure::new(format!(
            "materialize the web clip adapter: {error}"
        ))
    })?;
    Ok(dir.join("web_clip").join("web_clip_adapter.py"))
}

/// Look `name` up on `PATH`, returning an executable file if found.
pub(super) fn find_on_path(name: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|path| is_executable_file(path.as_path()))
}

#[cfg(unix)]
pub(super) fn is_executable_file(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.is_file()
        && path
            .metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

#[cfg(not(unix))]
pub(super) fn is_executable_file(path: &std::path::Path) -> bool {
    path.is_file()
}

/// Kill stragglers in the adapter's process group after the leader exits.
fn kill_stragglers(pid: u32) {
    #[cfg(unix)]
    {
        let target = format!("-{pid}");
        let _ = Command::new("kill").args(["-KILL", "--", &target]).output();
    }
}

/// Kill the whole adapter process tree on timeout.
fn kill_adapter_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let pid = child.id();
        let target = format!("-{pid}");
        let killed = Command::new("kill")
            .args(["-KILL", "--", &target])
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !killed {
            let _ = child.kill();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
    let _ = child.wait();
}

/// Spawn the adapter program with `args` and piped stdio.
///
/// A just-written executable can report a transient `ETXTBSY` under
/// concurrent load; that one error retries before giving up, so
/// script-based adapters stay reliable under `cargo test`.
fn spawn_adapter(
    program: &std::path::Path,
    args: &[OsString],
) -> AdapterResult<std::process::Child> {
    let mut attempts = 0;
    loop {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        match command.spawn() {
            Ok(child) => return Ok(child),
            Err(error)
                if error.raw_os_error() == Some(26) && attempts < 100 =>
            {
                attempts += 1;
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                return Err(AdapterFailure::new(format!(
                    "start the web clip adapter: {error}"
                )));
            }
        }
    }
}

/// Drain a child stdio pipe to a string; errors yield what arrived.
fn drain_pipe<R: Read>(pipe: Option<R>) -> String {
    let mut text = String::new();
    if let Some(mut pipe) = pipe {
        let _ = pipe.read_to_string(&mut text);
    }
    text
}

/// A non-zero exit or unparseable stdout is an adapter crash, reported
/// with the last 20 stderr lines.
fn adapter_crash(reason: String, stderr_text: String) -> AdapterFailure {
    let mut tail: Vec<&str> = stderr_text.lines().rev().take(20).collect();
    tail.reverse();
    let mut message = reason;
    if !tail.is_empty() {
        message.push('\n');
        message.push_str(&tail.join("\n"));
    }
    AdapterFailure::new(message)
}

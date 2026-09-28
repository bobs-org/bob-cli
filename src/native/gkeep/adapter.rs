//! `bob gkeep` adapter client: spawn the pinned Python Keep adapter.
//!
//! The client writes one compact JSON request to the adapter's stdin and
//! reads one JSON response from stdout. `BOB_GKEEP_ADAPTER` replaces the
//! `uv run --script …` invocation; it is the test hook, the same idea as
//! `BOB_CLIPBOARD_CMD`. Every request carries `"protocol": 1`, and the
//! master token travels on stdin only, never in argv.

use std::{
    ffi::OsString,
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use super::{
    config::GkeepConfig,
    model::{
        AdapterAuth, ArchiveRequest, ArchiveResponse, ArchiveResult,
        ArchiveTarget, ExchangeRequest, ExchangeResponse, KeepContent,
        KeepNote, PingRequest, PingResponse, SnapshotRequest, SnapshotResponse,
        ADAPTER_PROTOCOL_VERSION,
    },
    ui::Spinner,
};
use crate::native::gkeep::GkeepError;

/// The Keep credentials an adapter request carries (never in argv).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Credentials {
    email: String,
    master_token: String,
    device_id: String,
    state_path: PathBuf,
}

impl Credentials {
    /// Build credentials from the resolved config and a master token.
    pub(crate) fn from_config(
        config: &GkeepConfig,
        master_token: &str,
    ) -> Self {
        Self {
            email: config.email().to_string(),
            master_token: master_token.to_string(),
            device_id: config.device_id().to_string(),
            state_path: crate::native::env::bob_cli_cache_dir()
                .join("gkeep")
                .join("state.json"),
        }
    }

    fn auth(&self) -> AdapterAuth {
        AdapterAuth {
            email: self.email.clone(),
            master_token: self.master_token.clone(),
            device_id: self.device_id.clone(),
            state_path: self.state_path.to_string_lossy().into_owned(),
        }
    }
}

/// The resolved adapter invocation: a program plus its argv.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdapterClient {
    program: PathBuf,
    args: Vec<OsString>,
    timeout: Duration,
}

impl AdapterClient {
    /// Build a client around an explicit command and timeout.
    pub(crate) fn new(
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

    /// Resolve the adapter command: the `BOB_GKEEP_ADAPTER` override, or
    /// `uv run --quiet --script <materialized adapter>`.
    pub(crate) fn resolve(config: &GkeepConfig) -> Result<Self, GkeepError> {
        Self::resolve_with(
            config,
            GkeepConfig::adapter_override(),
            &find_on_path,
        )
    }

    fn resolve_with(
        config: &GkeepConfig,
        adapter_override: Option<PathBuf>,
        find_on_path: &dyn Fn(&str) -> Option<PathBuf>,
    ) -> Result<Self, GkeepError> {
        let timeout = Duration::from_secs(config.timeout_secs());
        if let Some(path) = adapter_override {
            return Ok(Self::new(path, Vec::new(), timeout));
        }
        let uv = find_on_path("uv").ok_or_else(|| {
            GkeepError::setup("uv", "uv was not found on PATH".to_string())
                .with_hint(
                    "install uv (https://docs.astral.sh/uv/) — bob gkeep runs \
                 its pinned Google Keep adapter with it",
                )
        })?;
        let script = materialized_adapter_script()?;
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

    /// Check adapter health: no auth, no network.
    pub(crate) fn ping(
        &self,
        spinner_label: Option<&str>,
    ) -> Result<PingResponse, GkeepError> {
        let (value, stderr_text) =
            self.run_request(&PingRequest::new(), spinner_label)?;
        let response = into_result(value, &stderr_text)?;
        let protocol =
            response.get("protocol").and_then(serde_json::Value::as_u64);
        if protocol != Some(u64::from(ADAPTER_PROTOCOL_VERSION)) {
            return Err(GkeepError::runtime(
                "protocol",
                format!(
                    "the Keep adapter spoke protocol {protocol:?}, expected {}",
                    ADAPTER_PROTOCOL_VERSION
                ),
            ));
        }
        serde_json::from_value(response).map_err(|error| {
            GkeepError::runtime(
                "protocol",
                format!("parse the ping response: {error}"),
            )
        })
    }

    /// List Home notes, plus Archive when `include_archived` is set.
    pub(crate) fn snapshot(
        &self,
        credentials: &Credentials,
        include_archived: bool,
        spinner_label: Option<&str>,
    ) -> Result<Vec<KeepNote>, GkeepError> {
        let request = SnapshotRequest {
            protocol: ADAPTER_PROTOCOL_VERSION,
            op: "snapshot",
            auth: credentials.auth(),
            include_archived,
        };
        let (value, stderr_text) = self.run_request(&request, spinner_label)?;
        let response = into_result(value, &stderr_text)?;
        let snapshot: SnapshotResponse = serde_json::from_value(response)
            .map_err(|error| {
                GkeepError::runtime(
                    "protocol",
                    format!("parse the snapshot response: {error}"),
                )
            })?;
        Ok(snapshot.notes)
    }

    /// Archive notes whose content still matches, in one guarded call.
    pub(crate) fn archive(
        &self,
        credentials: &Credentials,
        notes: &[(String, KeepContent)],
        spinner_label: Option<&str>,
    ) -> Result<Vec<ArchiveResult>, GkeepError> {
        let request = ArchiveRequest {
            protocol: ADAPTER_PROTOCOL_VERSION,
            op: "archive",
            auth: credentials.auth(),
            notes: notes
                .iter()
                .map(|(id, expect)| ArchiveTarget {
                    id: id.clone(),
                    expect: expect.clone(),
                })
                .collect(),
        };
        let (value, stderr_text) = self.run_request(&request, spinner_label)?;
        let response = into_result(value, &stderr_text)?;
        let archive: ArchiveResponse = serde_json::from_value(response)
            .map_err(|error| {
                GkeepError::runtime(
                    "protocol",
                    format!("parse the archive response: {error}"),
                )
            })?;
        Ok(archive.results)
    }

    /// Trade a sign-in cookie for a master token.
    pub(crate) fn exchange(
        &self,
        email: &str,
        cookie: &str,
        device_id: &str,
        spinner_label: Option<&str>,
    ) -> Result<String, GkeepError> {
        let request = ExchangeRequest {
            protocol: ADAPTER_PROTOCOL_VERSION,
            op: "exchange",
            email: email.to_string(),
            oauth_token: cookie.to_string(),
            device_id: device_id.to_string(),
        };
        let (value, stderr_text) = self.run_request(&request, spinner_label)?;
        let response = into_result(value, &stderr_text)?;
        let exchange: ExchangeResponse = serde_json::from_value(response)
            .map_err(|error| {
                GkeepError::runtime(
                    "protocol",
                    format!("parse the exchange response: {error}"),
                )
            })?;
        Ok(exchange.master_token)
    }

    /// Spawn the adapter, write one request, and read one response.
    ///
    /// Stdin, stdout, and stderr each drain on their own thread while the
    /// main thread polls `try_wait` against the configured deadline. The
    /// deadline starts before any blocking I/O, so a request larger than
    /// the pipe buffer cannot block past it when the child never reads.
    /// On timeout the whole adapter process group is killed, so an
    /// orphaned `uv` child (e.g. Python) cannot hold the pipes open and
    /// block the reader joins. Reported with `timed out after Ns`.
    /// After the leader exits, with any status, the rest of its process
    /// group is killed before joining the readers, so a straggler holding
    /// the pipes cannot hang the joins.
    fn run_request(
        &self,
        request: &impl serde::Serialize,
        spinner_label: Option<&str>,
    ) -> Result<(serde_json::Value, String), GkeepError> {
        let payload =
            serde_json::to_string(request).expect("adapter request serializes");
        let _spinner = spinner_label.map(Spinner::start);
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
                GkeepError::runtime(
                    "adapter",
                    format!("wait for the Keep adapter: {error}"),
                )
            })? {
                Some(status) => break status,
                None => {
                    if Instant::now() >= deadline {
                        kill_adapter_tree(&mut child);
                        let _ = stdin_writer.join();
                        let _ = stdout_reader.join();
                        let _ = stderr_reader.join();
                        return Err(GkeepError::runtime(
                            "timeout",
                            format!(
                                "the Keep adapter timed out after {}s",
                                self.timeout.as_secs()
                            ),
                        )
                        .with_hint(
                            "check your network connection, or raise \
                             gkeep.timeout_secs",
                        ));
                    }
                    thread::sleep(Duration::from_millis(10));
                }
            }
        };
        // The leader exited: kill any straggler holding the pipes
        // before joining the readers. Normally the group is empty.
        kill_stragglers(child.id());
        let stdin_error = stdin_writer.join().unwrap_or(None);
        let stdout_text = stdout_reader.join().unwrap_or_default();
        let stderr_text = stderr_reader.join().unwrap_or_default();
        match status.code() {
            Some(0) => {
                // The child exited cleanly without reading: ignore the
                // stdin EPIPE and evaluate stdout normally (garbage gives
                // "invalid JSON", a valid response succeeds).
            }
            Some(code) => {
                // A write failure is only reported when the child failed
                // with no stdout and no stderr: otherwise the exit status
                // and stderr report the real crash (an EPIPE merely means
                // the child exited before reading).
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
                    format!("the Keep adapter crashed (exit {code})"),
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
                    "the Keep adapter was terminated by a signal".to_string(),
                    stderr_text,
                ));
            }
        }
        let value: serde_json::Value = serde_json::from_str(&stdout_text)
            .map_err(|error| {
                adapter_crash(
                    format!("the Keep adapter returned invalid JSON: {error}"),
                    stderr_text.clone(),
                )
            })?;
        Ok((value, stderr_text))
    }
}

/// Kill stragglers in the adapter's process group after the leader exits.
///
/// The group leader is already reaped, so a failed `kill` (ESRCH) is
/// ignored. This never falls back to `child.kill()` on the reaped child.
fn kill_stragglers(pid: u32) {
    #[cfg(unix)]
    {
        let target = format!("-{pid}");
        let _ = Command::new("kill").args(["-KILL", "--", &target]).output();
    }
}

/// Kill the whole adapter process tree on timeout.
///
/// `uv run --script` starts Python as a child process; `child.kill()`
/// kills only `uv`, and the orphaned Python keeps the stdout/stderr
/// pipes open, so the reader joins block forever. The child runs in its
/// own process group (see `spawn_adapter`), so killing the negative pid
/// kills the group. Falls back to `child.kill()`, then waits.
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
/// A just-written executable can report a transient `ETXTBSY` ("Text
/// file busy", errno 26 on Linux and macOS) when executed under
/// concurrent load; that one error retries a few times before giving
/// up, so script-based adapters stay reliable under `cargo test`.
fn spawn_adapter(
    program: &std::path::Path,
    args: &[OsString],
) -> Result<std::process::Child, GkeepError> {
    let mut attempts = 0;
    loop {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("BOB_GKEEP_PARENT_PID", std::process::id().to_string());
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
                return Err(GkeepError::runtime(
                    "adapter",
                    format!("start the Keep adapter: {error}"),
                ));
            }
        }
    }
}

/// Materialize the embedded adapter script and return its cache path.
fn materialized_adapter_script() -> Result<PathBuf, GkeepError> {
    let dir = crate::runner::materialize_scripts().map_err(|error| {
        GkeepError::runtime(
            "adapter",
            format!("materialize the Keep adapter: {error}"),
        )
    })?;
    Ok(dir.join("gkeep").join("gkeep_adapter.py"))
}

/// Look `name` up on `PATH`, returning an executable file if found.
fn find_on_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|path| is_executable_file(path.as_path()))
}

#[cfg(unix)]
fn is_executable_file(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.is_file()
        && path
            .metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &std::path::Path) -> bool {
    path.is_file()
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
fn adapter_crash(reason: String, stderr_text: String) -> GkeepError {
    let mut tail: Vec<&str> = stderr_text.lines().rev().take(20).collect();
    tail.reverse();
    let mut message = reason;
    if !tail.is_empty() {
        message.push('\n');
        message.push_str(&tail.join("\n"));
    }
    GkeepError::runtime("adapter", message)
}

/// Split an `ok:false` response into a typed error with targeted hints.
///
/// An `internal` error carries the adapter's stderr tail (the last 20
/// lines, the same as `adapter_crash`), so the scrubbed traceback reaches
/// the user. Other kinds are unchanged.
fn into_result(
    value: serde_json::Value,
    stderr_text: &str,
) -> Result<serde_json::Value, GkeepError> {
    let ok = value
        .get("ok")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if ok {
        return Ok(value);
    }
    let kind = value
        .pointer("/error/kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("internal");
    let mut message = value
        .pointer("/error/message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("the Keep adapter reported an error")
        .to_string();
    if kind == "internal" {
        let mut tail: Vec<&str> = stderr_text.lines().rev().take(20).collect();
        tail.reverse();
        if !tail.is_empty() {
            message.push('\n');
            message.push_str(&tail.join("\n"));
        }
    }
    let hint = match kind {
        "auth" => Some("run `bob gkeep doctor`, then `bob gkeep login`"),
        "rate_limit" => Some("wait a few minutes"),
        "dependency" => Some("run `just check-adapter` or check uv"),
        "network" => Some("check your network connection and try again"),
        _ => None,
    };
    let mut error = GkeepError::runtime(kind, message);
    if let Some(hint) = hint {
        error = error.with_hint(hint);
    }
    Err(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_EMAIL: &str = "bryanbugyi34@gmail.com";
    const TEST_TOKEN: &str = "aas_et/test-master-token";

    fn test_config(timeout_secs: u64) -> GkeepConfig {
        GkeepConfig::for_tests(TEST_EMAIL, "3f9c0a1b2c3d4e5f", timeout_secs)
    }

    fn test_credentials() -> Credentials {
        Credentials::from_config(&test_config(300), TEST_TOKEN)
    }

    fn write_script(
        dir: &tempfile::TempDir,
        name: &str,
        body: &str,
    ) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, body).expect("write fake adapter script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&path)
                .expect("stat fake adapter script")
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&path, permissions)
                .expect("chmod fake adapter script");
        }
        path
    }

    fn responding_client(
        dir: &tempfile::TempDir,
        response: &str,
    ) -> AdapterClient {
        let path = write_script(
            dir,
            "adapter.sh",
            &format!("#!/bin/sh\ncat >/dev/null\nprintf '%s' '{response}'\n"),
        );
        AdapterClient::new(path, Vec::new(), Duration::from_secs(30))
    }

    fn sample_note_json() -> String {
        serde_json::json!({
            "id": "note-1",
            "kind": "note",
            "content": {
                "title": "Call dentist",
                "text": "They close at 5",
                "items": [],
            },
            "created": "2026-09-27T21:14:03Z",
            "edited": "2026-09-27T21:14:03Z",
        })
        .to_string()
    }

    #[test]
    fn ping_snapshot_archive_round_trip() {
        let dir = tempfile::tempdir().expect("temp dir");
        let note = sample_note_json();
        let path = write_script(
            &dir,
            "adapter.sh",
            &format!(
                "#!/bin/sh\n\
                 request=$(cat)\n\
                 case \"$request\" in\n\
                 *'\"op\":\"ping\"'*) printf '%s' \
                 '{{\"ok\":true,\"protocol\":1,\"python\":\"3.12.3\",\
                 \"gkeepapi\":\"0.17.1\",\"gpsoauth\":\"2.0.0\"}}';;\n\
                 *'\"op\":\"snapshot\"'*) printf '%s' \
                 '{{\"ok\":true,\"account\":\"{TEST_EMAIL}\",\"notes\":[{note}]}}';;\n\
                 *'\"op\":\"archive\"'*) printf '%s' \
                 '{{\"ok\":true,\"results\":[{{\"id\":\"note-1\",\
                 \"status\":\"archived\"}}]}}';;\n\
                 esac\n"
            ),
        );
        let client =
            AdapterClient::new(path, Vec::new(), Duration::from_secs(30));

        let ping = client.ping(None).expect("ping succeeds");
        assert_eq!(ping.gkeepapi, "0.17.1");

        let notes = client
            .snapshot(&test_credentials(), false, None)
            .expect("snapshot succeeds");
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].id, "note-1");
        assert_eq!(notes[0].content.title, "Call dentist");

        let content = notes[0].content.clone();
        let results = client
            .archive(
                &test_credentials(),
                &[("note-1".to_string(), content)],
                None,
            )
            .expect("archive succeeds");
        assert_eq!(results.len(), 1);
        assert!(results[0].status.is_success());
    }

    #[test]
    fn error_kinds_carry_messages_and_hints() {
        let dir = tempfile::tempdir().expect("temp dir");
        for (kind, hint) in [
            (
                "auth",
                Some("run `bob gkeep doctor`, then `bob gkeep login`"),
            ),
            ("rate_limit", Some("wait a few minutes")),
            ("dependency", Some("run `just check-adapter` or check uv")),
            (
                "network",
                Some("check your network connection and try again"),
            ),
            ("protocol", None),
            ("internal", None),
        ] {
            let response = serde_json::json!({
                "ok": false,
                "error": {"kind": kind, "message": format!("{kind} broke")},
            })
            .to_string();
            let client = responding_client(&dir, &response);
            let error = client.ping(None).expect_err("ping must fail");
            assert_eq!(error.exit_code(), 1, "{kind} exits 1");
            assert_eq!(error.kind(), kind);
            assert_eq!(error.message(), format!("{kind} broke"));
            assert_eq!(error.hint(), hint, "{kind} hint");
        }
    }

    #[test]
    fn crash_garbage_and_timeout() {
        let dir = tempfile::tempdir().expect("temp dir");

        let crashed = write_script(
            &dir,
            "crash.sh",
            "#!/bin/sh\necho 'adapter exploded' >&2\nexit 3\n",
        );
        let client =
            AdapterClient::new(crashed, Vec::new(), Duration::from_secs(30));
        let error = client.ping(None).expect_err("crash must fail");
        assert_eq!(error.kind(), "adapter");
        assert!(
            error.message().contains("crashed (exit 3)"),
            "crash names the exit: {}",
            error.message()
        );
        assert!(
            error.message().contains("adapter exploded"),
            "crash keeps stderr: {}",
            error.message()
        );

        let garbage = write_script(
            &dir,
            "garbage.sh",
            "#!/bin/sh\nprintf '%s' 'this is not json'\n",
        );
        let client =
            AdapterClient::new(garbage, Vec::new(), Duration::from_secs(30));
        let error = client.ping(None).expect_err("garbage must fail");
        assert_eq!(error.kind(), "adapter");
        assert!(error.message().contains("invalid JSON"));

        let slow = write_script(&dir, "slow.sh", "#!/bin/sh\nsleep 2\n");
        let client =
            AdapterClient::new(slow, Vec::new(), Duration::from_secs(1));
        let error = client.ping(None).expect_err("timeout must fail");
        assert_eq!(error.kind(), "timeout");
        assert!(
            error.message().contains("timed out after 1s"),
            "timeout names the deadline: {}",
            error.message()
        );
    }

    #[test]
    fn timeout_kills_grandchild_holding_stdout() {
        let dir = tempfile::tempdir().expect("temp dir");
        // The adapter backgrounds a grandchild that inherits stdout and
        // sleeps: killing only the direct child would leave the pipes
        // open and block the reader joins forever.
        let hanging = write_script(
            &dir,
            "hang.sh",
            "#!/bin/sh\n(sleep 30 & wait) &\nwait\n",
        );
        let client =
            AdapterClient::new(hanging, Vec::new(), Duration::from_secs(2));
        let start = Instant::now();
        let error = client.ping(None).expect_err("hang must time out");
        assert_eq!(error.kind(), "timeout");
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "timeout returns within a few seconds despite the grandchild",
        );
    }

    #[test]
    fn exiting_without_reading_reports_stdout_not_stdin() {
        let dir = tempfile::tempdir().expect("temp dir");
        // Exits 0 with garbage stdout without reading stdin: the stdin
        // EPIPE is ignored and stdout evaluates normally.
        let garbage = write_script(
            &dir,
            "fast-garbage.sh",
            "#!/bin/sh\nprintf '%s' 'not json'\nexit 0\n",
        );
        let client =
            AdapterClient::new(garbage, Vec::new(), Duration::from_secs(30));
        let error = client.ping(None).expect_err("garbage must fail");
        assert_eq!(error.kind(), "adapter");
        assert!(
            error.message().contains("invalid JSON"),
            "fast exit reports stdout: {}",
            error.message()
        );

        // Exits 0 with a valid response without reading stdin: success.
        let valid = write_script(
            &dir,
            "fast-valid.sh",
            "#!/bin/sh\nprintf '%s' '{\"ok\":true,\"protocol\":1,\"python\":\"3.12.3\",\"gkeepapi\":\"0.17.1\",\"gpsoauth\":\"2.0.0\"}'\nexit 0\n",
        );
        let client =
            AdapterClient::new(valid, Vec::new(), Duration::from_secs(30));
        let ping = client.ping(None).expect("fast valid succeeds");
        assert_eq!(ping.gkeepapi, "0.17.1");
    }

    #[test]
    fn token_travels_on_stdin_never_argv() {
        let dir = tempfile::tempdir().expect("temp dir");
        let stdin_path = dir.path().join("stdin.json");
        let argv_path = dir.path().join("argv");
        let path = write_script(
            &dir,
            "capture.sh",
            &format!(
                "#!/bin/sh\ncat > '{}'\nprintf '%s\\n' \"$@\" > '{}'\n\
                 printf '%s' '{{\"ok\":true,\"account\":\"{TEST_EMAIL}\",\"notes\":[]}}'\n",
                stdin_path.display(),
                argv_path.display(),
            ),
        );
        let client = AdapterClient::new(
            path.clone(),
            Vec::new(),
            Duration::from_secs(30),
        );
        client
            .snapshot(&test_credentials(), false, None)
            .expect("snapshot succeeds");

        let stdin_text =
            std::fs::read_to_string(&stdin_path).expect("read stdin dump");
        let argv_text =
            std::fs::read_to_string(&argv_path).expect("read argv dump");
        assert!(
            stdin_text.contains(TEST_TOKEN),
            "the token reaches the adapter on stdin"
        );
        assert!(
            !argv_text.contains(TEST_TOKEN)
                && !path.to_string_lossy().contains(TEST_TOKEN),
            "the token never appears in argv"
        );
    }

    #[test]
    fn every_request_carries_protocol_version() {
        let dir = tempfile::tempdir().expect("temp dir");
        let stdin_path = dir.path().join("stdin.json");
        let path = write_script(
            &dir,
            "capture.sh",
            &format!(
                "#!/bin/sh\n\
                 request=$(cat)\n\
                 printf '%s' \"$request\" > '{}'\n\
                 case \"$request\" in\n\
                 *'\"op\":\"ping\"'*) printf '%s' \
                 '{{\"ok\":true,\"protocol\":1,\"python\":\"3.12.3\",\
                 \"gkeepapi\":\"0.17.1\",\"gpsoauth\":\"2.0.0\"}}';;\n\
                 *'\"op\":\"exchange\"'*) printf '%s' \
                 '{{\"ok\":true,\"master_token\":\"aas_et/new\"}}';;\n\
                 *'\"op\":\"archive\"'*) printf '%s' \
                 '{{\"ok\":true,\"results\":[]}}';;\n\
                 *) printf '%s' \
                 '{{\"ok\":true,\"account\":\"{TEST_EMAIL}\",\"notes\":[]}}';;\n\
                 esac\n",
                stdin_path.display(),
            ),
        );
        let client =
            AdapterClient::new(path, Vec::new(), Duration::from_secs(30));
        let credentials = test_credentials();

        client.ping(None).expect("ping succeeds");
        assert_request_protocol(&stdin_path, "ping");
        client
            .snapshot(&credentials, true, None)
            .expect("snapshot succeeds");
        assert_request_protocol(&stdin_path, "snapshot");
        client
            .archive(&credentials, &[], None)
            .expect("archive succeeds");
        assert_request_protocol(&stdin_path, "archive");
        let token = client
            .exchange(TEST_EMAIL, "oauth2_4/cookie", "3f9c0a1b2c3d4e5f", None)
            .expect("exchange succeeds");
        assert_eq!(token, "aas_et/new");
        assert_request_protocol(&stdin_path, "exchange");
    }

    fn assert_request_protocol(stdin_path: &std::path::Path, op: &str) {
        let text =
            std::fs::read_to_string(stdin_path).expect("read stdin dump");
        let value: serde_json::Value =
            serde_json::from_str(&text).expect("request is JSON");
        assert_eq!(value["protocol"], serde_json::json!(1), "{op} protocol");
        assert_eq!(value["op"], serde_json::json!(op), "{op} name");
    }

    #[test]
    fn spawn_sets_parent_pid_env() {
        let dir = tempfile::tempdir().expect("temp dir");
        let pid_path = dir.path().join("parent_pid");
        let script = format!(
            "#!/bin/sh\ncat >/dev/null\nprintf '%s' {dollar}BOB_GKEEP_PARENT_PID > '{pid}'\nprintf '%s' '{{\"ok\":true,\"protocol\":1,\"python\":\"3.12.3\",\"gkeepapi\":\"0.17.1\",\"gpsoauth\":\"2.0.0\"}}'\n",
            dollar = "$",
            pid = pid_path.display(),
        );
        let path = write_script(&dir, "pid.sh", &script);
        let client =
            AdapterClient::new(path, Vec::new(), Duration::from_secs(30));
        client.ping(None).expect("ping succeeds");
        let recorded =
            std::fs::read_to_string(&pid_path).expect("read parent pid");
        assert_eq!(
            recorded,
            std::process::id().to_string(),
            "adapter sees the Rust parent pid"
        );
    }

    #[test]
    fn normal_exit_with_pipe_holding_straggler_returns_quickly() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write_script(
            &dir,
            "straggler.sh",
            "#!/bin/sh\nsleep 30 &\nprintf '%s' '{\"ok\":true,\"protocol\":1,\"python\":\"3.12.3\",\"gkeepapi\":\"0.17.1\",\"gpsoauth\":\"2.0.0\"}'\nexit 0\n",
        );
        let client =
            AdapterClient::new(path, Vec::new(), Duration::from_secs(2));
        let start = Instant::now();
        let ping = client.ping(None).expect("straggler still succeeds");
        assert_eq!(ping.gkeepapi, "0.17.1");
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "returns within a few seconds despite the straggler",
        );
    }

    #[test]
    fn internal_ok_false_carries_stderr_tail() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write_script(
            &dir,
            "internal.sh",
            "#!/bin/sh\necho 'MARKER_stderr_tail_123' >&2\nprintf '%s' '{\"ok\":false,\"error\":{\"kind\":\"internal\",\"message\":\"boom\"}}'\n",
        );
        let client =
            AdapterClient::new(path, Vec::new(), Duration::from_secs(30));
        let error = client.ping(None).expect_err("internal must fail");
        assert_eq!(error.kind(), "internal");
        assert!(
            error.message().contains("MARKER_stderr_tail_123"),
            "stderr tail reaches the user: {}",
            error.message()
        );
        // Non-internal kinds are unchanged (no stderr tail).
        let path = write_script(
            &dir,
            "auth.sh",
            "#!/bin/sh\necho 'MARKER_should_not_appear' >&2\nprintf '%s' '{\"ok\":false,\"error\":{\"kind\":\"auth\",\"message\":\"bad\"}}'\n",
        );
        let client =
            AdapterClient::new(path, Vec::new(), Duration::from_secs(30));
        let error = client.ping(None).expect_err("auth must fail");
        assert_eq!(error.kind(), "auth");
        assert!(
            !error.message().contains("MARKER_should_not_appear"),
            "auth keeps no tail: {}",
            error.message()
        );
    }

    #[test]
    fn large_request_with_fast_exit_succeeds() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write_script(
            &dir,
            "fast.sh",
            "#!/bin/sh\nprintf '%s' '{\"ok\":true,\"protocol\":1,\"python\":\"3.12.3\",\"gkeepapi\":\"0.17.1\",\"gpsoauth\":\"2.0.0\"}'\nexit 0\n",
        );
        let client =
            AdapterClient::new(path, Vec::new(), Duration::from_secs(30));
        let big = serde_json::json!({
            "protocol": 1,
            "op": "ping",
            "pad": "x".repeat(2 * 1024 * 1024),
        });
        let (value, _) =
            client.run_request(&big, None).expect("fast exit succeeds");
        assert_eq!(value["ok"], serde_json::json!(true));
    }

    #[test]
    fn large_request_with_hanging_adapter_times_out() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write_script(&dir, "hang.sh", "#!/bin/sh\nsleep 30\n");
        let client =
            AdapterClient::new(path, Vec::new(), Duration::from_secs(2));
        let big = serde_json::json!({
            "protocol": 1,
            "op": "ping",
            "pad": "y".repeat(2 * 1024 * 1024),
        });
        let start = Instant::now();
        let error = client
            .run_request(&big, None)
            .expect_err("hang must time out");
        assert_eq!(error.kind(), "timeout");
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "large hanging request times out quickly",
        );
    }

    #[test]
    fn resolve_uses_override_without_path_lookup() {
        let config = test_config(300);
        let override_path = PathBuf::from("/tmp/fake-gkeep-adapter");
        let client = AdapterClient::resolve_with(
            &config,
            Some(override_path.clone()),
            &|_| panic!("override must not touch PATH"),
        )
        .expect("override resolves");
        assert_eq!(client.program, override_path);
        assert!(client.args.is_empty());
        assert_eq!(client.timeout, Duration::from_secs(300));
    }

    #[test]
    fn resolve_without_uv_is_a_setup_error() {
        let config = test_config(60);
        let error = AdapterClient::resolve_with(&config, None, &|_| None)
            .expect_err("missing uv must fail");
        assert_eq!(error.exit_code(), 2);
        assert_eq!(error.kind(), "uv");
        assert!(
            error
                .hint()
                .unwrap_or_default()
                .contains("install uv (https://docs.astral.sh/uv/)"),
            "uv hint names the installer: {:?}",
            error.hint()
        );
    }
}

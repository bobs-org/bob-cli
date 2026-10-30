//! Web-clip adapter client behind `bob ref create`: spawn the pinned web-clip adapter.
//!
//! The client writes one compact JSON request to the adapter's stdin and
//! reads one JSON response from stdout. `BOB_WEB_CLIP_ADAPTER` replaces the
//! `uv run --quiet --script …` invocation; it is the test hook, the same
//! idea as `BOB_GKEEP_ADAPTER`. Every request carries `"protocol": 1`.
//! `BOB_WEB_CLIP_TIMEOUT_SECS` sets the overall adapter timeout.

use crate::native::env as bob_env;
use std::{
    collections::BTreeMap,
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
/// Per-item adapter budget sent with a `diagrams` request.
pub(super) const DIAGRAM_ITEM_TIMEOUT_SECS: u64 = 10;
/// Fixed overhead in the diagrams transport deadline
/// (`90s + 10s × unique items`, capped at [`DEFAULT_TIMEOUT_SECS`]).
const DIAGRAM_TIMEOUT_BASE_SECS: u64 = 90;
const NETWORK_TIMEOUT_HINT: &str =
    "check your network connection, or raise BOB_WEB_CLIP_TIMEOUT_SECS";
const DIAGRAM_TIMEOUT_HINT: &str =
    "a diagram may be too complex; diagrams print as source. Set \
     BOB_HIGHLIGHTS_DIAGRAMS=off to skip rendering, or raise \
     BOB_WEB_CLIP_TIMEOUT_SECS";

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
        Self::resolve_with(
            adapter_override(),
            timeout_secs(),
            &resolve_uv_lookup,
        )
    }

    /// Resolve a client with a fixed timeout (used by the doctor ping).
    pub(super) fn resolve_with_timeout(secs: u64) -> AdapterResult<Self> {
        Self::resolve_with(adapter_override(), secs, &resolve_uv_lookup)
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
                "install uv (https://docs.astral.sh/uv/) — bob ref \
                 create runs its pinned web capture adapter with it; uv is \
                 also searched in ~/.local/bin, ~/.cargo/bin, \
                 /opt/homebrew/bin, and /usr/local/bin",
            )
        })?;
        let script = materialized_adapter_script()?;
        if !script.is_file() {
            return Err(AdapterFailure::new(format!(
                "web clip adapter is not installed: {}",
                script.display()
            ))
            .with_hint(
                "reinstall bob so it materializes its embedded web clip \
                 adapter, or set BOB_WEB_CLIP_ADAPTER=/path/to/adapter",
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

    /// Resolve a client whose deadline matches the diagrams contract:
    /// `90s + 10s × unique items`, capped at [`DEFAULT_TIMEOUT_SECS`],
    /// unless `BOB_WEB_CLIP_TIMEOUT_SECS` is set.
    pub(super) fn resolve_for_diagrams(
        item_count: usize,
    ) -> AdapterResult<Self> {
        Self::resolve_with(
            adapter_override(),
            diagrams_timeout_secs(item_count),
            &resolve_uv_lookup,
        )
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

    /// Merge ordered article.json parts into one PDF.
    pub(super) fn compose(
        &self,
        request: &ComposeRequest,
    ) -> AdapterResult<ComposeSuccess> {
        let (value, stderr_text) = self.run_request(request)?;
        into_success(value, &stderr_text, "compose")
    }

    /// Render unique Mermaid sources in one adapter call.
    pub(super) fn diagrams(
        &self,
        request: &DiagramsRequest,
    ) -> AdapterResult<DiagramsSuccess> {
        let (value, stderr_text) =
            self.run_request_with_hint(request, DIAGRAM_TIMEOUT_HINT)?;
        into_success(value, &stderr_text, "diagrams")
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
        self.run_request_with_hint(request, NETWORK_TIMEOUT_HINT)
    }

    fn run_request_with_hint(
        &self,
        request: &impl Serialize,
        timeout_hint: &str,
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
                        .with_hint(timeout_hint));
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
    #[allow(dead_code)]
    #[serde(default)]
    pub(super) mermaid: Option<String>,
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

/// Web-clip table policy forwarded to the print adapter. Optional on the
/// protocol so older clients retain the adapter's auto/12 defaults.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(super) struct TablesSettings {
    pub(super) layout: String,
    pub(super) prose_words: u32,
}

/// One `capture` request.
#[derive(Debug, Clone, Serialize)]
pub(super) struct CaptureRequest {
    pub(super) protocol: u32,
    pub(super) op: &'static str,
    pub(super) url: String,
    pub(super) html_path: Option<String>,
    pub(super) workdir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) out_pdf: Option<String>,
    pub(super) dry_run: bool,
    pub(super) captured: String,
    pub(super) overrides: MetadataOverrides,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) render: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tables: Option<TablesSettings>,
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
        tables: TablesSettings,
    ) -> Self {
        Self {
            protocol: ADAPTER_PROTOCOL_VERSION,
            op: "capture",
            url,
            html_path,
            workdir,
            out_pdf: Some(out_pdf),
            dry_run,
            captured,
            overrides,
            render: None,
            tables: Some(tables),
        }
    }

    /// Capture a collection part to `article.json` without printing.
    pub(super) fn for_collection_part(
        url: String,
        workdir: String,
        dry_run: bool,
        captured: String,
    ) -> Self {
        Self {
            protocol: ADAPTER_PROTOCOL_VERSION,
            op: "capture",
            url,
            html_path: None,
            workdir,
            out_pdf: None,
            dry_run,
            captured,
            overrides: MetadataOverrides::default(),
            render: Some(false),
            tables: None,
        }
    }
}

/// One article.json path in a `compose` request.
#[derive(Debug, Clone, Serialize)]
pub(super) struct ComposePart {
    pub(super) article_json: String,
}

/// Collection-level metadata forwarded to `compose`.
#[derive(Debug, Clone, Serialize)]
pub(super) struct CollectionMeta {
    pub(super) title: String,
    pub(super) author: Option<String>,
    pub(super) published: Option<String>,
    pub(super) captured: String,
}

/// One `compose` request: ordered article.json parts into one PDF.
#[derive(Debug, Clone, Serialize)]
pub(super) struct ComposeRequest {
    pub(super) protocol: u32,
    pub(super) op: &'static str,
    pub(super) parts: Vec<ComposePart>,
    pub(super) collection: CollectionMeta,
    pub(super) workdir: String,
    pub(super) out_pdf: String,
    pub(super) tables: Option<TablesSettings>,
}

impl ComposeRequest {
    pub(super) fn new(
        parts: Vec<ComposePart>,
        collection: CollectionMeta,
        workdir: String,
        out_pdf: String,
        tables: TablesSettings,
    ) -> Self {
        Self {
            protocol: ADAPTER_PROTOCOL_VERSION,
            op: "compose",
            parts,
            collection,
            workdir,
            out_pdf,
            tables: Some(tables),
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
    pub(super) source_html: Option<String>,
    #[serde(default)]
    pub(super) code_anchors: Option<String>,
    #[serde(default)]
    pub(super) article_json: Option<String>,
    #[serde(default)]
    pub(super) tables: Option<serde_json::Value>,
    #[serde(default)]
    pub(super) warnings: Vec<String>,
}

/// One `{id, source}` item in a `diagrams` request.
#[derive(Debug, Clone, Serialize)]
pub(super) struct DiagramItem {
    pub(super) id: String,
    pub(super) source: String,
}

/// One `diagrams` request: unique Mermaid sources in one Chrome page.
#[derive(Debug, Clone, Serialize)]
pub(super) struct DiagramsRequest {
    pub(super) protocol: u32,
    pub(super) op: &'static str,
    pub(super) workdir: String,
    pub(super) items: Vec<DiagramItem>,
    pub(super) item_timeout_s: u64,
}

impl DiagramsRequest {
    pub(super) fn new(workdir: String, items: Vec<DiagramItem>) -> Self {
        Self {
            protocol: ADAPTER_PROTOCOL_VERSION,
            op: "diagrams",
            workdir,
            items,
            item_timeout_s: DIAGRAM_ITEM_TIMEOUT_SECS,
        }
    }
}

/// Per-item result from a successful `diagrams` op.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct DiagramItemResult {
    pub(super) id: String,
    pub(super) ok: bool,
    #[serde(default)]
    pub(super) pdf: Option<String>,
    #[serde(default)]
    pub(super) width_pt: Option<f64>,
    #[serde(default)]
    pub(super) height_pt: Option<f64>,
    #[serde(default)]
    pub(super) error: Option<String>,
}

/// A successful `diagrams` response.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub(super) struct DiagramsSuccess {
    #[serde(default)]
    pub(super) protocol: u32,
    #[serde(default)]
    pub(super) ok: Option<bool>,
    #[serde(default)]
    pub(super) op: Option<String>,
    #[serde(default)]
    pub(super) mermaid: Option<String>,
    #[serde(default)]
    pub(super) browser: Option<BrowserInfo>,
    #[serde(default)]
    pub(super) results: Vec<DiagramItemResult>,
    #[serde(default)]
    pub(super) warnings: Vec<String>,
}

/// A successful `compose` response.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub(super) struct ComposeSuccess {
    #[serde(default)]
    pub(super) protocol: u32,
    #[serde(default)]
    pub(super) ok: Option<bool>,
    #[serde(default)]
    pub(super) op: Option<String>,
    #[serde(default)]
    pub(super) pdf_bytes: Option<u64>,
    #[serde(default)]
    pub(super) source_html: Option<String>,
    #[serde(default)]
    pub(super) code_anchors: Option<String>,
    #[serde(default)]
    pub(super) tables: Option<serde_json::Value>,
    #[serde(default)]
    pub(super) images: ImageReport,
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
    bob_env::var_os(ENV_ADAPTER_OVERRIDE)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn timeout_secs() -> u64 {
    bob_env::var_os(ENV_TIMEOUT_SECS)
        .and_then(|value| value.into_string().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
}

/// Diagrams transport deadline. `BOB_WEB_CLIP_TIMEOUT_SECS` wins when set.
pub(super) fn diagrams_timeout_secs(item_count: usize) -> u64 {
    if bob_env::var_os(ENV_TIMEOUT_SECS)
        .filter(|value| !value.is_empty())
        .is_some()
    {
        return timeout_secs();
    }
    DIAGRAM_TIMEOUT_BASE_SECS
        .saturating_add(
            DIAGRAM_ITEM_TIMEOUT_SECS.saturating_mul(item_count as u64),
        )
        .min(DEFAULT_TIMEOUT_SECS)
        .max(1)
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
/// Shared with the `curl` and `Xvfb` lookups; `uv` itself resolves through
/// [`crate::native::env::resolve_uv`], which also checks the well-known
/// install locations outside `PATH`.
pub(super) fn find_on_path(name: &str) -> Option<PathBuf> {
    crate::native::env::find_on_path(name)
}

/// The `uv` lookup used for adapter resolution: the shared
/// [`crate::native::env::resolve_uv`] (PATH plus the well-known install
/// locations); anything else falls back to a plain PATH lookup.
fn resolve_uv_lookup(name: &str) -> Option<PathBuf> {
    if name == "uv" {
        crate::native::env::resolve_uv().map(|(path, _)| path)
    } else {
        find_on_path(name)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn sample_capture() -> CaptureRequest {
        CaptureRequest::new(
            "https://example.com/a".into(),
            None,
            "/tmp/w".into(),
            "/tmp/w/out.pdf".into(),
            false,
            "2026-10-15".into(),
            MetadataOverrides::default(),
            TablesSettings {
                layout: "auto".into(),
                prose_words: 12,
            },
        )
    }

    #[test]
    fn single_capture_request_json_is_byte_identical() {
        let json = serde_json::to_string(&sample_capture())
            .expect("serialize capture request");
        assert_eq!(
            json,
            r#"{"protocol":1,"op":"capture","url":"https://example.com/a","html_path":null,"workdir":"/tmp/w","out_pdf":"/tmp/w/out.pdf","dry_run":false,"captured":"2026-10-15","overrides":{},"tables":{"layout":"auto","prose_words":12}}"#
        );
    }

    #[test]
    fn capture_request_omits_out_pdf_when_render_is_false() {
        let mut request = sample_capture();
        request.out_pdf = None;
        request.render = Some(false);
        let json = serde_json::to_string(&request)
            .expect("serialize render:false capture");
        assert_eq!(
            json,
            r#"{"protocol":1,"op":"capture","url":"https://example.com/a","html_path":null,"workdir":"/tmp/w","dry_run":false,"captured":"2026-10-15","overrides":{},"render":false,"tables":{"layout":"auto","prose_words":12}}"#
        );
    }

    #[test]
    fn compose_request_serializes_null_collection_fields() {
        let request = ComposeRequest::new(
            vec![ComposePart {
                article_json: "/abs/part-1/article.json".into(),
            }],
            CollectionMeta {
                title: "Tokio Internals".into(),
                author: None,
                published: None,
                captured: "2026-10-15".into(),
            },
            "/abs/compose".into(),
            "/abs/collection.pdf".into(),
            TablesSettings {
                layout: "records".into(),
                prose_words: 20,
            },
        );
        let json =
            serde_json::to_string(&request).expect("serialize compose request");
        assert_eq!(
            json,
            r#"{"protocol":1,"op":"compose","parts":[{"article_json":"/abs/part-1/article.json"}],"collection":{"title":"Tokio Internals","author":null,"published":null,"captured":"2026-10-15"},"workdir":"/abs/compose","out_pdf":"/abs/collection.pdf","tables":{"layout":"records","prose_words":20}}"#
        );
    }

    #[test]
    fn compose_success_deserializes() {
        let value: ComposeSuccess = serde_json::from_str(
            r#"{"protocol":1,"ok":true,"op":"compose","pdf_bytes":1234,"source_html":"/tmp/render/source.html","code_anchors":"/tmp/render/code-anchors.json","images":{"total":2,"kept":2,"skipped_small":0,"failed":0},"warnings":[]}"#,
        )
        .expect("deserialize compose success");
        assert_eq!(value.pdf_bytes, Some(1234));
        assert_eq!(
            value.source_html.as_deref(),
            Some("/tmp/render/source.html")
        );
        assert_eq!(
            value.code_anchors.as_deref(),
            Some("/tmp/render/code-anchors.json")
        );
        assert_eq!(value.images.kept, 2);
        assert!(value.tables.is_none());
        assert!(value.warnings.is_empty());
    }

    #[test]
    fn compose_success_defaults_new_render_paths() {
        let value: ComposeSuccess = serde_json::from_str(
            r#"{"protocol":1,"ok":true,"op":"compose","pdf_bytes":1234}"#,
        )
        .expect("deserialize older compose response");
        assert!(value.source_html.is_none());
        assert!(value.code_anchors.is_none());
        assert!(value.tables.is_none());
    }

    #[test]
    fn malformed_optional_tables_report_does_not_break_response() {
        let value: ComposeSuccess = serde_json::from_str(
            r#"{"protocol":1,"ok":true,"op":"compose","pdf_bytes":1234,"tables":{"version":"future"}}"#,
        )
        .expect("deserialize success with malformed optional table report");
        assert!(value.tables.is_some());
    }

    #[test]
    fn diagrams_request_json_is_byte_identical() {
        let request = DiagramsRequest::new(
            "/abs/scratch/diagrams".into(),
            vec![DiagramItem {
                id: "a".repeat(40),
                source: "flowchart LR\n  A --> B".into(),
            }],
        );
        let json = serde_json::to_string(&request)
            .expect("serialize diagrams request");
        assert_eq!(
            json,
            format!(
                r#"{{"protocol":1,"op":"diagrams","workdir":"/abs/scratch/diagrams","items":[{{"id":"{}","source":"flowchart LR\n  A --> B"}}],"item_timeout_s":10}}"#,
                "a".repeat(40)
            )
        );
    }

    #[test]
    fn diagrams_success_deserializes_mixed_results() {
        let value: DiagramsSuccess = serde_json::from_str(
            r#"{"protocol":1,"ok":true,"op":"diagrams","mermaid":"12.2.0","browser":{"kind":"chrome","path":"/opt/chrome","version":"129"},"results":[{"id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","ok":true,"pdf":"/abs/a.pdf","width_pt":200.0,"height_pt":100.0},{"id":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","ok":false,"error":"Parse error on line 4: bad"}],"warnings":[]}"#,
        )
        .expect("deserialize diagrams success");
        assert_eq!(value.mermaid.as_deref(), Some("12.2.0"));
        assert_eq!(value.results.len(), 2);
        assert!(value.results[0].ok);
        assert_eq!(value.results[0].width_pt, Some(200.0));
        assert!(!value.results[1].ok);
        assert_eq!(
            value.results[1].error.as_deref(),
            Some("Parse error on line 4: bad")
        );
    }

    #[test]
    fn ping_response_reads_mermaid_version() {
        let value: PingResponse = serde_json::from_str(
            r#"{"protocol":1,"ok":true,"op":"ping","mermaid":"12.2.0"}"#,
        )
        .expect("deserialize ping");
        assert_eq!(value.mermaid.as_deref(), Some("12.2.0"));
    }

    #[test]
    fn diagrams_timeout_uses_item_budget_unless_env_overrides() {
        let _guard =
            crate::native::env::TestEnvGuard::set(&[(ENV_TIMEOUT_SECS, None)]);
        assert_eq!(diagrams_timeout_secs(0), 90);
        assert_eq!(diagrams_timeout_secs(1), 100);
        assert_eq!(diagrams_timeout_secs(21), 300);
        assert_eq!(diagrams_timeout_secs(50), 300);
        let _guard = crate::native::env::TestEnvGuard::set(&[(
            ENV_TIMEOUT_SECS,
            Some(OsStr::new("12")),
        )]);
        assert_eq!(diagrams_timeout_secs(8), 12);
    }

    #[test]
    fn capture_success_reads_article_json() {
        let value: CaptureSuccess = serde_json::from_str(
            r#"{"protocol":1,"ok":true,"op":"capture","kind":"article","pdf_bytes":null,"article_json":"/tmp/w/article.json","source_html":"/tmp/w/render/source.html","code_anchors":"/tmp/w/render/code-anchors.json","warnings":[],"images":{},"fidelity":{},"capture":{},"metadata_sources":{},"word_count":0}"#,
        )
        .expect("deserialize capture success");
        assert_eq!(value.article_json.as_deref(), Some("/tmp/w/article.json"));
        assert_eq!(
            value.source_html.as_deref(),
            Some("/tmp/w/render/source.html")
        );
        assert_eq!(
            value.code_anchors.as_deref(),
            Some("/tmp/w/render/code-anchors.json")
        );
        assert_eq!(value.pdf_bytes, None);
    }

    #[test]
    fn capture_success_defaults_new_render_paths() {
        let value: CaptureSuccess = serde_json::from_str(
            r#"{"protocol":1,"ok":true,"op":"capture","kind":"article"}"#,
        )
        .expect("deserialize older capture response");
        assert!(value.source_html.is_none());
        assert!(value.code_anchors.is_none());
    }

    #[test]
    fn capture_success_preserves_gist_metadata_and_provenance() {
        let value: CaptureSuccess = serde_json::from_str(
            r#"{"protocol":1,"ok":true,"op":"capture","kind":"article","final_url":"https://gist.github.com/karpathy/442a6bf","title":"LLM Wiki","author":"karpathy","metadata_sources":{"title":"gist-heading","author":"gist-owner"},"warnings":[],"images":{},"fidelity":{},"capture":{},"word_count":0}"#,
        )
        .expect("deserialize Gist capture response");
        assert_eq!(value.title.as_deref(), Some("LLM Wiki"));
        assert_eq!(value.author.as_deref(), Some("karpathy"));
        assert_eq!(
            value.metadata_sources.get("title").map(String::as_str),
            Some("gist-heading")
        );
        assert_eq!(
            value.metadata_sources.get("author").map(String::as_str),
            Some("gist-owner")
        );
    }
}
